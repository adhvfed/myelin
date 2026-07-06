# UX: git browsing, commits & PR flows

_The Git surface has a solid, accessible presentation layer (semantic tokens, glyph+text diff signals, first-class loading/empty states in most places), but it fails badly on task completeness and IA at exactly the product's stated flagship. Pull requests are effectively unreachable (no list, no links), the PR overview is missing its entire reason to exist — the diff and the linked-issue/CI/doc/discussion context pane that git.md G-6/G-7 and the ws-a switch test build the whole wedge around — and directory browsing is a dead end that makes most real repos un-navigable. Secondary gaps (hardcoded 'main' ref, a checks error masquerading as PR-not-available, leaked raw error strings, no branch switcher/blame) compound the sense that only the happy path of a top-level file on the default branch actually works._

**Kept findings:** 13  (🔴 3 critical  ·  🟠 2 high  ·  🟡 5 medium  ·  🔵 3 low)

---

### 1. 🔴 Pull requests are undiscoverable — no PR list and no link from anywhere in the UI

- **Severity:** critical  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-ia
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/index.tsx:62`

**What:** The only PR route is prs/[n].tsx, reachable exclusively by typing a numeric PR id into the URL. There is no prs/index.tsx list route, and no surface links to a PR: the repo home (index.tsx:64-69) links only to Commits and to blob files; the repos list, commit log, commit diff and blob pages carry no PR entry point.

**Impact:** A user cannot find, browse, or open a pull request through the UI without out-of-band knowledge of a PR number. The flagship G-6 surface has no front door.

**Fix:** Add a repo-level PR index (state pill, author, head→base, review count) and link to it from the repo home header and app nav, mirroring the Commits link at index.tsx:66.

> _Verifier note:_ find over the git route tree returns only prs/[n].tsx — no prs/index.tsx. index.tsx:64-69 renders only CloneUrl + a Commits link; blob links at :86. AppShell.tsx NAV (lines 23-28) has Code/Issues/Chat/CI/Knowledge — no PR entry. The inbox 'A pull request needs your review' (AppShell.tsx:280-282) is a static <span>, not a link. Confirmed.

### 2. 🔴 No PR diff / files-changed surface — the funnel-target diff (G-7) does not exist for PRs

- **Severity:** critical  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/prs/[n].tsx:78`

**What:** git.md:24-35 designates G-7 (diff/files-changed) as the funnel-target dense engineer surface. The PR overview (prs/[n].tsx) renders only a checks panel and a merge-readiness card — no files-changed list, no diff, and no link to one. A commit-level diff route exists (commit/[oid].tsx) but nothing connects a PR to it, and there is no head_ref-vs-base_ref diff route.

**Impact:** A reviewer landing on a PR cannot view what changed — the core review action — defeating the stated purpose of the wedge flagship.

**Fix:** Add a PR files-changed/diff route (head_oid vs base_ref) reusing FileDiff/DiffRow from commit/[oid].tsx, linked prominently from the PR overview header.

> _Verifier note:_ prs/[n].tsx body (lines 66-85) renders heading + state pill + one-line head→base summary + ChecksAndMerge + a GT-004b footnote. No diff/files-changed anywhere. git.md:24 confirms G-7 as funnel target. PrVM (api.ts:80-89) does carry head_oid/base_ref, so a diff route is buildable but absent. Confirmed.

### 3. 🔴 PR overview is missing the entire context pane — no description, linked issue/run/doc, discussion, or commits

- **Severity:** critical  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/prs/[n].tsx:65`

**What:** git.md:11-22 defines G-6's whole value as the context pane where the PR's linked issue/run/doc resolve into it as chips/unfurls. The rendered page shows only heading + state pill, a one-line head→base/author/reviews summary (lines 73-76), the checks panel, and the merge card. There is no PR title/body, no linked-issue chip, no discussion thread, and no commits-in-PR list.

**Impact:** The flagship surface delivers none of its differentiating job; a reviewer gets less context than GitHub's PR tab, not more.

**Fix:** Build the context pane: PR title/body, linked-issue/run/doc chips (R-09), a discussion thread, and a commits-in-PR list; even skeleton slots (R-13 A.2) would satisfy the IA.

> _Verifier note:_ prs/[n].tsx:66-85 confirmed — no title/body/discussion/commits/linked-issue elements. Reinforced at the data layer: PrVM (api.ts:80-89) has only number, pr_state, base_ref, head_ref, head_oid, author, reviews, durable — no title/body/linked-item fields, so the pane is absent end-to-end. Confirmed.

### 4. 🟠 Directories are dead ends — subdirectory files are unreachable from the UI

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/index.tsx:79`

**What:** In the repo tree, directory entries render as plain non-clickable text (index.tsx:79-84, fallback of the is_dir Show) with the caption 'Browsing into directories is a follow-on (GT-004b)' (line 96). The blob route accepts only a single path segment (blob/[ref]/[path].tsx:3-4 comment) so nested paths cannot be addressed. Only top-level files are linked (line 86).

**Impact:** Any repo with a src/ or nested layout is un-browsable: clicking a folder does nothing and files inside can never be opened, breaking G-2 code browsing for essentially all real repos.

**Fix:** Make directory rows navigate into a tree view at that path/ref and support nested path segments (catch-all param) in the blob route; at minimum disclose the limitation as a disabled affordance.

> _Verifier note:_ index.tsx:77-90 — is_dir true renders a plain <span> (no <A>), false renders the linked file. Caption at :95-96 confirms it is a known follow-on. blob/[ref]/[path].tsx:3-4 comment confirms single-segment path. Impact is real; noting the team explicitly scoped it to GT-004b. Confirmed.

### 5. 🟠 A checks-projection failure is rendered as 'This pull request is not available'

- **Severity:** high  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/prs/[n].tsx:53`

**What:** getPrChecks is a separate createAsync (lines 38-42) hitting a distinct endpoint (/prs/{n}/checks) but resolved inside the same ErrorBoundary whose fallback (lines 53-58) reads 'This pull request is not available' with a gate icon. If the PR record loads but the checks endpoint errors, the whole overview is replaced by that no-access/gate message.

**Impact:** A transient checks-backend error masquerades as a permission-denied/missing-PR state, telling the reviewer the PR doesn't exist when it does — violating the R-21 no-access-vs-error distinction.

**Fix:** Wrap the ChecksAndMerge block in its own ErrorBoundary with a 'Checks unavailable — reconnecting' message (G-9 stale/reconnecting, git.md:46) so a checks failure degrades locally.

> _Verifier note:_ checks() (createAsync :38-42) is read at line 78 inside the shared ErrorBoundary (:53-91); a rejected checks promise surfaces via Suspense to the fallback at :53-58 whose copy is 'This pull request is not available' + gate icon. getPr and getPrChecks are separate edge calls (api.ts:164-179), so independent checks failure is realistic. Confirmed.

### 6. 🟡 Ref is hardcoded to 'main' across repo home and commit breadcrumb

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/index.tsx:66`

**What:** The repo home links Commits to /commits/main (index.tsx:66) and every tree file to /blob/main/... (:86); the commit page breadcrumb links 'commits' to /commits/main (commit/[oid].tsx:34). RepoHomeVM carries no default_branch field, so 'main' is an unverified assumption, and the commit breadcrumb ignores the ref the commit was reached from.

**Impact:** For repos whose default branch is master/develop/trunk the Commits and file links 404. Even in main-default repos, viewing /commits/{other-ref} then clicking a commit yields a breadcrumb that returns to /commits/main, not the ref you came from — a general wayfinding bug.

**Fix:** Add default_branch to RepoHomeVM and use it for these links; derive the commit breadcrumb ref from the commit's actual navigation context rather than hardcoding 'main'.

> _Verifier note:_ Confirmed hardcoded 'main' at index.tsx:66, :72 ('on main'), :86, and commit/[oid].tsx:34. api.ts RepoHomeVM (lines 15-21) has no default_branch. Severity lowered from high to medium: the product steers users to main (empty-state prints `git push -u origin main`, index.tsx:58) and there is no branch switcher yet, so non-main-default repos are barely a supported path; the always-load-bearing part is the breadcrumb-loses-ref wayfinding bug.

### 7. 🟡 Raw internal error strings leaked to users; no no-access vs not-found vs error distinction

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/commit/[oid].tsx:39`

**What:** Error fallbacks across the surface render String(err.message ?? err) inline: commit/[oid].tsx:40, commits/[ref].tsx:40, blob/[ref]/[path].tsx:35, repos/index.tsx:25, and prs/[n].tsx:56. R-21 (git.md:18,46,54) calls for distinct, dignified states — permission-denied as a no-access card, no-results vs no-access as separate states — not a dumped exception message.

**Impact:** A 403, 404, and 500 all look identical and can surface backend detail; users get no guidance on next action.

**Fix:** Map status codes to distinct humanised states (no-access, not-found, generic retryable error) and stop rendering raw err.message to end users.

> _Verifier note:_ Verified String(err.message ?? err) at all five cited fallbacks. The 401 case is handled centrally (api.ts authed() redirects to /login), but 403/404/500 all fall through to the raw-message fallbacks. The GDPR-leak framing is somewhat speculative (depends what the gateway puts in err.message), but the R-21 undifferentiated-error-state gap is real. Confirmed, medium.

### 8. 🟡 No branch/tag switcher anywhere — non-URL-typed branches are unreachable

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-ia
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/commits/[ref].tsx:33`

**What:** Every Git surface is pinned to a single ref carried in the URL; there is no branch/tag picker on repo home, blob, or commit-log pages. The commit-log header displays the ref name (commits/[ref].tsx:33-35) but offers no way to change it. G-1 (git.md:50) lists branches/tags as repo-home content.

**Impact:** Users cannot discover which branches exist or switch to one; viewing any non-default branch requires hand-editing the URL.

**Fix:** Add a ref switcher (branches + tags) on repo home and carry it through commits/blob/diff routes.

> _Verifier note:_ Confirmed: no picker in any git route. commits/[ref].tsx:33-35 renders the ref as static text. index.tsx has no branch UI. git.md:50 lists 'branches/tags' under G-1. Confirmed, medium.

### 9. 🟡 CI checks are inert labels, not links — the failing-check → step → line chain is broken

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/prs/[n].tsx:127`

**What:** Each required check renders as icon + status text + context name (prs/[n].tsx:127-135) with no link to the underlying run/step/failing line. git.md G-9 §10 (line 47) requires 'one click check→step→line warm (CA-1)' and W4 is 'failing check → step → line → fix' (git.md:34).

**Impact:** A red required check is a dead end: the reviewer sees it failed but cannot navigate to the run or failing step/line to act, so the advertised warm-chain flow is unreachable.

**Fix:** Link each check context to its CI run (and, where available, the failing step/line) so the panel becomes the entry into the fix flow.

> _Verifier note:_ Confirmed at prs/[n].tsx:118-137 — each <li> is a static span (cue icon/label) + <code>ctx</code> + 'required' text, no <A>/href. PrChecksVM (api.ts:92-100) carries only context-name string arrays, no run refs, so the link target isn't even in the data yet. git.md:47 confirms the requirement. Confirmed, medium.

### 10. 🟡 Blob view has no blame, file history, backlinks, or raw view (G-2 job E5 incomplete)

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/blob/[ref]/[path].tsx:53`

**What:** The blob page renders read-only numbered contents (blob/[ref]/[path].tsx:53-69) and nothing else. G-2 (git.md:51) specifies blame with the W5 backlink trail 'blame → commit → PR → issue → decision' plus graceful LFS/binary handling. None of blame, per-file history, backlinks, raw view, or binary/large-file handling is present; contents.split('\n') on a binary would dump garbled text.

**Impact:** The E5 'understand this line's history' job cannot be completed, and a binary/LFS file produces broken output rather than a graceful fallback.

**Fix:** Add blame with commit/PR/issue backlinks, a file-history link, a raw view, and binary/large-file detection with a download fallback.

> _Verifier note:_ Confirmed at blob/[ref]/[path].tsx:41-72 — only file.path header, base_oid line, and a <For> over contents.split('\n'). BlobVM (api.ts:29-35) is path/contents/base_oid/viewer_may_edit — no blame/binary/size fields. git.md:51 confirms G-2 requirements. Note much of this is scoped as GT-004b follow-on per the file's header comment, but the binary-garbling and missing-backlinks gaps are real. Confirmed, medium.

### 11. 🔵 Repos-list empty state is a blank statement, not onboarding-forward

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/index.tsx:35`

**What:** When a tenant has no repos the list shows only 'No repositories in this tenant yet.' (index.tsx:34-38). The per-repo empty state (repo home index.tsx:53-59) is onboarding-forward, but the tenant-list-level empty state is a bare statement.

**Impact:** A brand-new tenant lands with no on-page path to create/import a repo — a squandered first-run moment.

**Fix:** Add a create/import-repo CTA and a one-line 'push your first repo' hint to the empty repos list, mirroring the per-repo empty state.

> _Verifier note:_ Confirmed at repos/index.tsx:34-38 (data-testid repos-empty, plain <p>). Severity lowered from medium to low: git.md:50's G-1 empty-state spec explicitly concerns a repo's empty state ('a new repo's empty state teaches the next action'), which the per-repo screen already satisfies; the tenant-list empty state is not directly specced, and repos are created via push (no in-UI create flow exists yet), so this is a minor enhancement rather than a spec violation.

### 12. 🔵 Commit-log pagination is one-directional with no position feedback

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/commits/[ref].tsx:72`

**What:** The log offers only an 'Older commits' cursor link (commits/[ref].tsx:72-81). There is no 'Newer'/previous link and no page/position indicator; the cursor replaces the URL so returning requires the browser back button.

**Impact:** A user paging deep into history has no in-page way back to newer commits and no sense of where they are.

**Fix:** Add a 'Newer commits' link (or prev/next pair) and a lightweight position indicator.

> _Verifier note:_ Confirmed: commits/[ref].tsx:72-81 renders a single 'Older commits' <A> gated on page.page.next_cursor; no prev link, no position readout. CommitsPage.page (api.ts) exposes only next_cursor/limit, so a 'newer' link would need URL history or an added prev cursor. Confirmed, low.

### 13. 🔵 Blob breadcrumb drops ref and path — no trail back to the containing tree

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-ia
- **Location:** `frontend/apps/web/src/routes/(app)/git/repos/[repo]/blob/[ref]/[path].tsx:26`

**What:** The blob breadcrumb is only Repositories / repo (blob/[ref]/[path].tsx:26-30); it omits the ref and the file's path segments. The header shows '@ ref' (line 47) as static text but there is no clickable trail back to the tree at that ref or to parent directories.

**Impact:** From a file the user cannot step back up the tree via the breadcrumb; combined with the missing directory pages this leaves the file view without upward navigation.

**Fix:** Include the ref and path segments (as links to the corresponding tree views) in the breadcrumb once tree navigation exists.

> _Verifier note:_ Confirmed at blob/[ref]/[path].tsx:26-30 — breadcrumb <nav> has only the Repositories and repo links. The @ref at :47 is a non-clickable <span>. Dependent on tree-navigation (finding 4) existing. Confirmed, low.
