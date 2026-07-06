# Review — 2026-07-06

Multi-agent review of the Myelin platform: **24 units** reviewed and adversarially verified.

**Totals:** 85 kept findings (76 confirmed)  ·  🔴 3 critical  ·  🟠 11 high  ·  🟡 30 medium  ·  🔵 37 low  ·  ⚪ 4 nit

- [Executive summary](00-executive-summary.md)
- [**Delta: impact of the 26 post-review commits (CT track + MR-009b)**](DELTA-post-CT-and-MR009b.md) — no finding fixed; several escalated to live; 4 new findings (3 HIGH) on the CI prod-exec + git-wire surfaces

## Codebase

| Unit | Findings | Top sev |
|---|---|---|
| [Issue tracker (myelin-issues)](codebase-issues.md) | 1 | 🟡 medium |
| [Storage layer (myelin-storage)](codebase-storage.md) | 3 | 🔵 low |
| [Git hosting (myelin-git)](codebase-git.md) | 2 | 🟠 high |
| [Identity, access & tenancy (myelin-identity-service, myelin-identity, myelin-tenancy)](codebase-identity.md) | 3 | 🟡 medium |
| [Search (myelin-search)](codebase-search.md) | 2 | 🟠 high |
| [Knowledge platform (myelin-knowledge)](codebase-knowledge.md) | 2 | 🟡 medium |
| [Chat (myelin-chat, myelin-chat-gateway)](codebase-chat.md) | 2 | 🔵 low |
| [GDPR engine (myelin-gdpr-service, myelin-gdpr, myelin-gdpr-macros)](codebase-gdpr.md) | 2 | 🟠 high |
| [Notifications (myelin-notif)](codebase-notif.md) | 4 | 🟡 medium |
| [Cross-artifact refs (myelin-refs-service, myelin-refs)](codebase-refs.md) | 3 | 🟡 medium |
| [CI (myelin-ci-controlplane, myelin-ci-dispatch, myelin-ci-sandbox)](codebase-ci.md) | 3 | 🟠 high |
| [Flow engine (myelin-flow)](codebase-flow.md) | 5 | 🟠 high |
| [Agent fabric (myelin-agent-service, myelin-agent)](codebase-agent.md) | 1 | 🟡 medium |
| [Event bus & substrate (myelin-events, myelin-substrate)](codebase-events.md) | 1 | 🟡 medium |
| [Control plane & query (myelin-control-plane, myelin-query)](codebase-controlplane.md) | 3 | 🟡 medium |
| [External surface (myelin-edge, myelin-cli, myelin-mcp, myelin-client)](codebase-edge.md) | 5 | 🟡 medium |
| [Support crates (myelin-harness, myelin-lints, myelin-content, myelin-config)](codebase-support.md) | 2 | 🟡 medium |
| [Cross-cutting: tenant isolation & authorization propagation](codebase-xc-tenancy.md) | 2 | 🟡 medium |
| [Cross-cutting: personal-data lifecycle & erasure](codebase-xc-gdpr.md) | 2 | 🔵 low |
| [Frontend web app (correctness + security)](codebase-fe-web.md) | 4 | 🟡 medium |
| [Design system & overlay primitives (correctness + a11y)](codebase-fe-ds.md) | 7 | 🟡 medium |

## UX

| Unit | Findings | Top sev |
|---|---|---|
| [UX: first-run, login & the one-shell](ux-ux-firstrun.md) | 6 | 🟠 high |
| [UX: git browsing, commits & PR flows](ux-ux-git.md) | 13 | 🔴 critical |
| [UX: accessibility & visual craft](ux-ux-a11y-visual.md) | 7 | 🟠 high |
