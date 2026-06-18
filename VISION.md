# Myelin — Vision & Canonical Brief

> This document is the single source of truth for *what Myelin is and why it exists*.
> Every planning and implementation agent must read this before starting work and must
> not contradict it. If reality forces a deviation, the deviation must be written down
> and justified — silence is not allowed.

## 1. What Myelin is

Myelin is an **EU-sovereign software delivery platform**. It is a direct response to the
desire for European digital sovereignty: it gives software companies everything they need
to build and manage their projects **in a GDPR-safe, EU-controlled manner**, without
depending on US-controlled hyperscalers or SaaS vendors.

Myelin is the connective tissue ("myelin sheath") between the tools an engineering
organisation uses. The differentiator is not any single tool — it is that all the tools
share one identity model, one permission model, one event bus, and one agent fabric, so
that work flows between them without friction and **autonomous agents are first-class
citizens, not bolt-ons**.

## 2. The subsystems

Myelin is one platform composed of these subsystems:

1. **Git hosting** — repository hosting, code browsing, pull/merge requests, code review.
2. **Continuous Integration (CI)** — pipelines triggered by repository and platform events.
3. **Issue tracker** — serves *engineers and product managers alike*; supports the
   workflows corporations require (roadmaps, sprints, hierarchies, custom fields, SLAs,
   reporting, audit).
4. **Knowledge platform** — rich-text content, tables, lists, folders, databases
   (Notion-class), so an organisation can host its knowledge inside the platform.
5. **Chat** — conversation tool that can **reference any other artifact** in the system
   (a commit, an issue, a doc, a CI run) and lets humans and agents talk to each other in
   the same channels.

These are unified by **shared backend system(s)**: identity & access, the event bus,
the agent fabric, storage, search, notifications, and the cross-artifact reference graph.

## 3. Non-negotiable principles

- **World-scalable from day 1.** Architecture decisions assume global scale and
  multi-tenancy from the outset. "World-scale means world-scale" — do not shy away from
  the necessary technical complexity.
- **Top-of-the-line UX and design.** This is a product, not an internal tool. UX and
  visual design are first-class requirements at every layer.
- **Agent-native from the ground up.** The platform is *designed for agents*. This means
  first-class **event propagation and triggers** across all subsystems. During
  development we do **not** integrate real agents — we build the setup and **mock
  implementations**. Use the **strategy pattern** everywhere agents plug in so that
  switching from mock to real agents is trivial (a config/implementation swap, not a
  rewrite).
- **GDPR-safe & EU-sovereign by construction.** Data residency, data subject rights
  (access/erasure/portability), lawful-basis tracking, auditability, and the ability to
  run entirely on EU-controlled infrastructure are architectural constraints, not
  features bolted on later.
- **Quality over plan-adherence.** The plan is a tool. The goal is high-quality software
  that meets the platform's needs. When the plan and reality diverge, choose quality and
  write down why.
- **Honesty about uncertainty.** Agents must be explicit about what they are unsure of,
  what they assumed, and what they deferred.

## 4. Technology steer (not a mandate)

The repository's `.gitignore` is pre-seeded for **Rust/Cargo**, including
`cargo-mutants` (mutation testing). Treat this as a **strong steer toward Rust** for the
performance- and correctness-critical backend, with mutation testing as part of the
quality bar. This is a steer, not a cage: each architecture agent owns its own internal
tech choices and may diverge **if** it justifies the divergence in writing and keeps it
consistent with the platform's shared systems and goals. Frontend/UX stack is open and
to be decided in the architecture phases.

## 5. The planning & build process

Work proceeds in numbered phases under `planning/`. Each phase consumes the previous one
and is committed + pushed when done.

1. `planning/01-research` — research: personas, competitive landscape & positioning, a
   comprehensive use-case catalogue (including non-obvious cases), and a broad technical
   structuring of the platform and the glue between subsystems. Broad, structured,
   addresses every concern; upfront about uncertainty.
2. `planning/02-holistic-architecture` — high-level architecture: how the systems are
   structured and built, the tech, the views and CLI commands required, usage examples,
   and how subsystems interact as a whole. Not yet full implementation detail.
3. `planning/03-shared-systems-architecture` — detailed technical roadmap for the
   **shared** backend system(s) the subsystems depend on.
4. `planning/04-subsystem-architectures/<subsystem>/` — one agent per identified
   subsystem produces a detailed technical spec. Agents sketch in numbered folders
   before committing to a final design in an `architecture/` folder. Subsystems are
   sequenced when one strongly influences another (fully sequential is acceptable —
   architecture comes before process). Changes required in shared systems are specified,
   and the subsystem spec is written on those premises.
5. `planning/05-refined-shared-systems-architecture` — a reconciliation agent reviews all
   architectures as a whole, refines the shared systems, and **rewrites all of the `04`
   documents from scratch** with the necessary adjustments. Also specifies a **testing
   strategy** for the system as a whole and in parts.
6. `planning/06-roadmaps/<system>/*.md` — one agent per subsystem and shared system
   produces an implementation roadmap. Does not shy away from complexity.
7. `planning/07-prompts/*` — agent(s) convert the roadmaps into **one sequence of
   prompts** (target 400k–700k tokens total) that operationalise all roadmaps into coding
   (and, if required, research) tasks. Chunks are split/combined so each prompt can be fed
   to an agent with clean context. Each prompt instructs the agent to commit when done.
8. **Execution** — run each prompt in sequence, **one agent at a time**, to completion +
   commit, until all prompts are done. Sequential so agents can use the testing strategy
   fully and adapt when plans meet reality. Between agents, the orchestrator decides
   whether intermediate work is needed (e.g. platform capabilities for developing agents,
   or gaps discovered) and launches intermediate agents as required.

## 6. Definition of done for a planning phase

- All deliverables are markdown under the correct `planning/NN-*` path.
- Every concern in scope is addressed (breadth over depth where depth isn't yet due).
- Open questions, assumptions, and risks are listed explicitly.
- Cross-references to other docs/subsystems are made where relevant.
- Work is committed and pushed.
