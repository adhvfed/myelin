# Engineering Insights — Doctrine for Building Myelin

This folder distills **hard-won engineering doctrine** for building a world-scale,
multi-tenant, agent-native developer platform of Myelin's shape. It is drawn from prior
art and real-world experience with systems of this kind. The failure modes described here
are the ones such platforms *actually hit* — not hypotheticals — and the principles are the
ones that, in retrospect, people wish they had followed from the first commit.

## How to use these documents

- Treat every item as a **default you should follow**, in the same spirit as
  [`../VISION.md`](../VISION.md): you may deviate, but only with a reason written down.
  Silence is not allowed.
- `VISION.md` says *what* we are building and *why*. These documents say *how to build it
  well* and *how it tends to go wrong*. Read the relevant one before the corresponding
  planning or implementation work.
- These are insights, not a blueprint of an existing system. Where they name a concrete
  approach, it is because that approach is **settled industry practice** for the problem —
  not because you must copy any particular implementation. You are designing Myelin from
  first principles; use these to avoid re-paying for lessons that are already paid.

## The specificity contract (important)

These docs deliberately vary how prescriptive they are:

- **Where the answer is settled** — durability, tenancy, identity, sandboxing, the event
  backbone — they name the proven approach directly, because rediscovering it is expensive
  and the design space is narrow. Follow it unless you can write down why it's wrong here.
- **Where the design space is genuinely open** — the collaborative editor, the UX details,
  per-subsystem storage choices — they give you the principles and the failure modes and
  **leave the design to you**. If you can find something better, do; that is the point of
  building clean.

## The honesty rule that underpins everything

The single most common way platforms like this deceive themselves is the gap between an
**ambitious design** and a **shipped floor**. Shipping a floor is fine. Shipping a floor
that *masquerades as done* is the failure. So: **name your floors.** If a thing is partial,
untested, or deferred, say so in writing and name the follow-on. Untested is acceptable if
you say it's untested; silent skipping is the failure mode. This rule recurs throughout
these documents because it is the one that keeps the whole effort honest.

## Index

| Doc | Read it before… | What it covers |
|-----|-----------------|----------------|
| [`01-process-and-quality-doctrine.md`](01-process-and-quality-doctrine.md) | any build phase | How to sequence work, gate it, and prove it; why the code outranks the docs |
| [`02-platform-substrate.md`](02-platform-substrate.md) | shared-systems & subsystem architecture | The shared backbone every subsystem stands on: identity, events, causality, the reference graph, tenancy, storage, durability |
| [`03-agent-native-fabric.md`](03-agent-native-fabric.md) | designing the agent fabric / triggers | Making agents first-class with no carve-out; the strategy boundary; the event→trigger→action pipeline; safety |
| [`04-hard-problems.md`](04-hard-problems.md) | committing to any subsystem design | The genuinely unsolved or expensive problems, named honestly, so you plan for them instead of being surprised |
| [`05-ux-and-design.md`](05-ux-and-design.md) | any frontend design or build | Top-tier UX as architecture, the primitives to build first, and the quantified cost of retrofitting design |
