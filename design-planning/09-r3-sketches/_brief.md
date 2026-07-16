# R3 sketch pack — shared brief (design-before-frontend, VISION §3)

> Date 2026-07-16 · Phase R3 (Git/PR UX + first-run), ledger
> `planning/system-reviews/2026-06-26/14-release-track-ledger.md` §R3.
> This is NOT a re-divergence. The design direction is FROZEN: the 08 design system
> (`design-planning/08-design-system/DESIGN-MANUAL.md` + `REFINEMENTS.md`) and the 6c winning
> direction (`design-planning/06-design-sketches/6c-finalists/finalist-A-instrument/`). An R3
> sketch is an **implementation-ready screen spec**: exact IA, full state set, data contract,
> keyboard/SR behavior — the thing a builder codes from without inventing anything.

## What each sketch deliverable is

One directory per surface, containing:

1. **`sketch*.html`** — self-contained HTML with inline `<style>`, linking `../tokens.css`
   (the generated design-system tokens — semantic vars only, no hex, no physical left/right;
   logical properties). One file per major state where states change layout (at minimum:
   populated, empty, loading-skeleton, no-access, error). Realistic Myelin dogfood content
   (this repo's own PRs/branches/checks — e.g. the R2 authz PRs), never lorem ipsum; at least
   one German or French string per surface.
2. **`NOTES.md`** — the spec the HTML can't carry:
   - **IA + routes** (SolidStart paths under `frontend/apps/web/src/routes/`),
   - **full R-21 state enumeration** (incl. states not sketched and why),
   - **data contract**: every rendered field tagged `EXISTING:<VM.field>` (from
     `frontend/apps/web/src/lib/api.ts`) or `NEW:<proposed endpoint + field>` — the NEW list
     is the backend work order for the build wave,
   - **keyboard map + SR behavior** (what announces, landmark structure),
   - **component reuse**: which `design-planning/08-design-system/02-components/*` and which
     existing design-system primitives (Dialog/Popover/Menu/Tooltip/Toast/Icon) it uses; any
     genuinely NEW shared primitive must be named and justified (expect: Skeleton, Chip,
     StatusPill, Button — check REFINEMENTS first),
   - **open questions** for the orchestrator gate, honestly named (VISION: floors are fine,
     masquerading isn't).

## Binding rails (violations fail the gate)

From the manual (read it — these are the ones R3 reviews already caught the app violating):
- Focus ring on EVERY interactive element incl. autofocused inputs — never `outline:none`
  inline (manual §6, must-ship #5).
- Active nav = `--surface-hover` fill + brighter text; accent NEVER as a full nav fill (R1
  binding, §7). Accent is rationed; never accent-colored body/link text below AA on its
  surface — commit oids/links use `--text-primary` or `--info`+underline (§3.1).
- Loading = structure-matching skeleton + `aria-busy` + one debounced polite live region —
  no spinners, no "Loading…" text (§5.3, must-ship #4).
- No color/background via inline style on interactive elements (hover must be CSS-able,
  §7 PROVEN don't). Sketches should express interactive color via classes.
- Status by glyph+label+position, never color alone (CI, PR state, review verdicts).
- Primary CTA rides `--c-btn-primary-bg`/`--c-btn-primary-text`, not raw `--accent`.
- Dense-but-calm; hierarchy via weight/color before size; 4/8px ramp; borders/layered
  surfaces over shadow; agents render with the four-channel agent treatment, no sparkle.
- Empty states teach the next action (R-20); no-access vs not-found vs error are DISTINCT
  dignified states, never a raw `err.message` (R-21).
- `100dvh` not `100vh` for full-height chrome.
- All three themes must work (link tokens.css and test `data-theme`); RTL-safe (logical
  properties); German expansion tolerated on labels.

## Surface specs (per git.md — read `design-planning/05-user-facing-surfaces/git.md` in full)

The G-spec DoD + switch tests in git.md are the acceptance contract per surface. The ux review
findings each sketch must dissolve are in `reviews/2026-07-06/ux-ux-git.md`,
`ux-ux-firstrun.md`, `ux-ux-a11y-visual.md` (cited per assignment).

## Current app reality (continuity constraints)

- Frontend is **SolidJS/SolidStart** at `frontend/apps/web` (older docs say React — code wins).
- Shell: `src/components/AppShell.tsx` (header + icon rail + secondaryNav slot + main). The
  G-6 context pane needs a **fourth shell region** — sketching it is in the PR-overview
  assignment's scope; keep it a shell-owned slot, not a per-screen hack.
- Existing routes/VMs: see `src/lib/api.ts` (RepoHomeVM, BlobVM, CommitRowVM, CommitDiffVM,
  DiffFileVM, PrVM, PrChecksVM) and `src/routes/(app)/git/…`. `gate_admitted` from the checks
  projection is AUTHORITATIVE for merge readiness — UI never recomputes policy.
- Icons: the 42-icon library only (`frontend/packages/design-system/src/icon-names.ts`);
  PR-relevant names exist (`pull-request, merge, gate, check-pass, check-fail, check-pending,
  commit, repo, folder, file, link, chevron`). Need a new icon → name it in NOTES, don't draw ad hoc.

## Backend gaps already censused (tag these NEW fields consistently)

PR list GET (query logic exists, unexposed) · PR head-vs-base diff · PR `title`/`body` ·
PR discussion/comments (NO store exists yet — R3 scopes one; design it) · linked
issue/run/doc refs for a PR (refs graph exists, no edge surface) · commits-in-PR ·
tree-at-path + nested blob path · branches/tags list + `default_branch` · check→run refs.
