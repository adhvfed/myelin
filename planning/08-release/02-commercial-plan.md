# Commercial Plan — positioning, model, GTM, viability gates

_2026-07-06. Founder is solo, Norway-based, bootstrapping, with grant funding named as an open variable.
This document makes the open commercial calls with rationale; every decision is reversible and dated.
Where a decision needs founder override, it says so — silence means the plan stands._

## 1. Positioning

**Category:** EU-sovereign software delivery platform (Git hosting + CI, growing into issues/chat/docs).

**One-liner:** *"The European software forge: Git, CI, and issues on EU soil, GDPR-safe by construction,
built for teams where AI agents are first-class developers."*

**Why this framing wins for a solo entrant:**
- "GitHub alternative" is unwinnable head-on; "**sovereign** GitHub+CI alternative" is a real, growing
  procurement category (Schrems II fallout, NIS2, EU public-sector cloud policies, US-cloud distrust).
  Buyers in this category tolerate smaller vendors *because sovereignty is the point*.
- The GDPR engine is not a checkbox — erasure/portability/residency are architectural. Competitors
  (GitLab self-managed, Forgejo/Gitea, Codeberg) offer *location*; Myelin offers *construction*. The R6.5
  move — generating the compliance paper pack from the live data-map — is a demo no competitor has.
- **Agent-nativeness is the differentiator, not the wedge.** Lead sales conversations with sovereignty
  (budget exists today); demo the agent fabric (MCP-governed agents with real HITL, audit, and budgets)
  as the "and it's built for what's coming" close. The build-in-public story — *this platform was built
  by agents driving it* — is itself the marketing proof.

**Anti-positioning (what we refuse to claim at GA):** not "world-scale today" (architecture yes,
operations one cell), not a Notion/Slack replacement yet, not an agent-hosting cloud yet.

## 2. ICP (initial customer profile)

**Primary:** EU software consultancies and product SMEs, **5–50 developers**, in or selling into
regulated sectors (public-sector suppliers, health, fintech, energy) — starting with **Norway + DACH**,
where the founder's network and the sovereignty pressure are strongest. They currently pay GitHub/GitLab,
feel procurement/compliance friction, and have a named person responsible for GDPR.

**Secondary (pull, don't push):** AI-forward dev teams who want governed agent access to their forge —
they arrive via the build-in-public channel.

**Explicitly later:** enterprises >250 seats (procurement cycles a solo founder can't survive),
hobbyists at scale (support load without revenue).

## 3. Licensing & business model — DECISION

**Open-core under FSL (Functional Source License), with a hosted EU cloud as the primary revenue line.**

- **Core platform: FSL-1.1** (source-available, converts to Apache-2.0 after 2 years). Rationale:
  - The sovereignty ICP *requires* source access and an exit story — a closed-source solo vendor is
    uninsurable risk for them; source-available + self-host packaging (R5.4) neutralizes both the
    bus-factor and lock-in objections, which are the two hardest objections this company faces.
  - FSL (vs AGPL) blocks the one existential threat — a hyperscaler or larger EU host reselling Myelin
    as a service — while staying honest-open enough for grant eligibility and community trust. (If
    NLnet/NGI review flags FSL as insufficient for a given grant, the fallback is AGPL for the
    grant-funded components; decide per-grant, not globally.)
  - **Founder override point:** pure-AGPL (maximal commons credibility, weaker moat) vs FSL. Plan says FSL.
- **Revenue lines, in order of activation:**
  1. **Hosted EU cloud** (Tier B free → GA paid): the default motion, zero-friction.
  2. **Supported self-host** (annual contract: updates channel, priority fixes, upgrade assistance) —
     activated on first inbound demand, not marketed before GA.
  3. *(post-GA)* Enterprise tier: SSO/SAML enforcement, audit export, compliance pack, multi-cell.

## 4. Pricing (GA; Tier B is free)

- **Free:** up to 3 users, 1 GB, community CI minutes cap. Exists for evaluation and the funnel, kept
  deliberately small (solo support load).
- **Team: €12 / user / month** (annual: €10) — Git + CI + issues, EU residency, GDPR tooling, MCP/agent
  access. Anchored between GitHub Team (~$4) and GitLab Premium (~$29): sovereignty + compliance justify
  a premium over GitHub; undercutting GitLab Premium keeps the "switch" math easy.
- **CI compute metered** beyond an included pool (the flow-engine budget/metering work is the billing
  substrate — R3.7 makes it billing-grade). Metered CI is the natural expansion revenue.
- Billing via Stripe (EU entity, see §7). Invoice + SEPA for the ICP's finance departments at GA.

**Viability arithmetic (why this can work solo):** at €12/user, a 15-dev customer ≈ €2.2k/yr. The €100k/yr
"founder replaces salary" line ≈ **~45 such teams or ~700 paid seats** — reachable inside the ICP without
a sales team, *if* churn stays low (sovereignty customers are sticky) and infra cost per tenant stays
under ~15% of revenue (single-cell economics say yes).

## 5. GTM sequence (mirrors the R-track tiers)

**Now → Tier D (build-in-public foundation):**
- Start the public narrative immediately: a monthly engineering letter (the reviews, ledgers, and
  attestation gates are extraordinary content — publish them lightly redacted). Channels: personal
  blog/RSS + Mastodon/Bluesky + Hacker News when a piece earns it. One hour/week, no more.
- Register the waitlist page (name, email, company size, current forge, sovereignty driver — the last
  field qualifies the pipeline for free).

**Tier D → Tier B (design partners):**
- Recruit **3–5 design partners** from the founder's Norwegian/DACH network — consultancies are ideal
  (multi-project, feedback-rich, and they *resell* trust later). Offer: free through beta + locked 50%
  year-one discount + roadmap voice. Ask: weekly usage, honest feedback, a quotable case study at GA.
- The design-partner agreement (R6.1) carries the honesty: no SLA, data export guaranteed, self-host
  escape hatch.

**Tier B → GA:**
- GA scope is **Git + CI + Issues** (chat/knowledge labeled beta). Rationale: three excellent tools beat
  five half tools; the VISION's full package remains the destination, the direction memory's sequencing
  already agrees.
- Launch: case studies + the agent-native demo video (local Claude driving a governed merge through MCP
  with HITL) + Show HN / European tech press (the sovereignty angle has media pull in 2026–27).
- Public-sector track opens *after* GA using the EN 301 549 self-assessment (R6.7) — procurement
  timelines mean seeds planted at GA harvest in 2028.

## 6. Funding — DECISION

**Bootstrap + apply for EU digital-commons grants now, in parallel.** Do not raise VC pre-GA.
- **NGI Zero Commons / NLnet** (€5k–50k, rolling calls) and **Sovereign Tech Fund** (larger, targets
  infrastructure commons): Myelin's open components (the GDPR engine, the sandbox attestation harness,
  the agent-governance fabric) are exactly their shape. Applications are grant-per-component, which fits
  FSL-core + open-components licensing. **Action: draft the NLnet application during R1 — calls close
  and review takes months; the grant should land near Tier B when the pentest (R6.3) needs paying for.**
- Grants fund the two things bootstrapping can't: the external pentest (~€10–20k) and founder runway
  insurance. VC is deferred with a written trigger: >€10k MRR + a reason to hire (support load or a
  second cell), not before — the sovereignty ICP also *prefers* a non-VC vendor story.

## 7. Solo-dev operating reality (the honest constraints)

- **Legal entity:** Norwegian AS before Tier B (external data ⇒ liability shield + DPA counterparty).
  ToS/privacy/DPA from a specialist template provider reviewed once by a lawyer (budget ~€3–5k, grant
  or bootstrap).
- **Support:** email + a public issue tracker (Myelin's own — dogfood as support portal). Published
  response target: 1 business day, no 24/7 claim. GA stays EU-timezone honest.
- **On-call, sized for one human:** the R5.2 alerting pages the founder; the blast-radius answer is the
  R5.3 rehearsed rollback + R4.3 scheduled restore drills, not heroics. A written "founder unavailable"
  runbook (a trusted person can execute restore-and-notify) partially answers the bus-factor question;
  source-available answers the rest.
- **Time budget rule:** ≥70% build until Tier B, then ≥50% build / 30% partners / 20% narrative. The
  R-track gates, not the calendar, decide tier promotion.
- **Cost floor pre-revenue:** one production cell on Scaleway/Hetzner + managed PG/S3 ≈ €150–400/mo;
  status page, e-mail, domain ≈ €50/mo. Trivial against runway; the real cost is founder time.

## 8. Viability gates (numbers that gate tier promotion)

| Gate | Threshold | Meaning |
|---|---|---|
| Tier D → recruit partners | Founder 4 weeks GitHub-free (R4 exit) | The product can hold a real workload |
| Tier B → GA | ≥3 design partners with ≥4 active weeks each; ≥1 says "we'd pay"; zero cross-tenant incidents; support load <5 h/wk | External demand + operational headroom exist |
| GA + 6 months — continue/adjust | ≥10 paying teams **or** ≥€2.5k MRR; logo churn <10%/quarter; infra <20% of MRR | The wedge is real; keep going |
| GA + 6 months — pivot trigger | <4 paying teams and flat waitlist | Reposition (likely: lead with agent-native, or verticalize on public-sector suppliers) — decided then, with data |

## 9. Top commercial risks, honestly

1. **Breadth vs one person.** Five subsystems is a company-sized surface. Mitigation is the GA-scope cut
   (Git+CI+Issues) and refusing feature-breadth sales pressure until revenue funds help.
2. **Trust asymmetry**: nobody moves their source code to a solo vendor lightly. Mitigation: source-
   available, self-host packaging, export-by-construction, design partners as references, published
   security reviews (this repo's review culture is a *sellable asset* — keep publishing it).
3. **Incumbent sovereignty-washing**: GitHub/GitLab EU-datacenter offerings blunt the location argument.
   Mitigation: sell *construction* (GDPR engine, attestations, agent governance), not location — location
   is table stakes we also have.
4. **The agent-native bet arrives late**: if governed-agent demand matures slower than hoped, the
   sovereignty wedge must carry revenue alone. It can (see §4 arithmetic) — agent-native is upside,
   not the plan's load-bearing wall.
5. **Founder burnout/bus-factor**: the gates include support-load ceilings; the runbook + mirror +
   source-availability are the honest answer to "what if you disappear."

## 10. Immediate commercial actions (this month, alongside R0)

1. Reserve the entity name / domain set; confirm "Myelin" trademark availability in software services
   (EUIPO search — rename now is cheap, at GA is not). **Founder action.**
2. Stand up the waitlist page + first build-in-public post (the 2026-07-06 review makes a strong,
   honest first post: "we reviewed our own platform with 24 adversarial agents; here's what they found").
3. Draft the NLnet/NGI Zero application (agent can draft; founder submits).
4. Shortlist 8–10 design-partner candidates from the network so recruiting at Tier D is a send, not a search.
