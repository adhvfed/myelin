# Design system & overlay primitives (correctness + a11y)

_The overlay substrate is well-architected: focus/trap/scroll-lock/inert/Escape-stack are written once in overlay-core.ts and composed by every primitive, and the tests genuinely assert a11y behavior (trap wrap-around, Escape+return-focus, scroll-lock apply/restore, roving tabindex, type-ahead, live-region roles) rather than just rendering. Two real cross-primitive defects stand out: a modal's inert-background silences and traps the Toast layer that is deliberately stacked above it, and the positioner computes a viewport clamp (maxBlockSize) that no consumer applies, so long menus/popovers overflow off-screen. A few reactivity and APG-conformance sharp edges round out the list. Nothing is critical, but the inert-toast and overflow issues are user-visible a11y gaps that the current tests do not cover._

**Kept findings:** 7  (🟡 2 medium  ·  🔵 4 low  ·  ⚪ 1 nit)

---

### 1. 🟡 A modal's inert background silences and traps the Toast layer stacked above it

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** accessibility
- **Location:** `frontend/packages/design-system/src/overlays/primitives/overlay-core.ts:103`

**What:** hideOthers() (overlay-core.ts:103-124) iterates document.body.children and sets [inert] on every child that does not contain the modal panel. ToastProvider renders its role=region notification host through OverlayPortal (Toast.tsx), and OverlayPortal (primitives/OverlayPortal.tsx) mounts via Solid <Portal mount={document.body}>, so the toast region lives in its own direct child <div> of body, mounted at app root before any modal opens. When Dialog/ConfirmDialog opens (modal:true -> unhide = hideOthers(panel) in createOverlay.ts:64), that toast portal div is inert-ed even though z-toast > z-modal (asserted in substrate.test.tsx) paints it visually above the modal. inert removes it from the a11y tree and tab order, so a role=status/alert toast raised while a modal is open is not announced (WCAG 4.1.3) and its Undo/Dismiss buttons cannot be reached by keyboard or F6.

**Impact:** A toast fired while a Dialog/Confirm is open (autosave notice, background job, a persistent danger toast, or an Undo affordance) is silent to screen-reader users and unreachable by keyboard. The visual layer (toast on top) and the a11y tree (toast inert/hidden) directly contradict each other.

**Fix:** Exclude the toast portal from hideOthers: have ToastProvider tag its portal root with a data attribute and skip that child in the hideOthers loop (or skip any child at the z-toast layer). Add a test that opens a Dialog, shows a toast, and asserts the toast region is not [inert] and its Undo button is focusable.

> _Verifier note:_ Confirmed in source: hideOthers loops body.children and inert-s all not containing contentEl (overlay-core.ts:106-115); OverlayPortal uses Portal mount=document.body (each portal = a distinct body child div); ToastProvider's region is inside an OverlayPortal (Toast.tsx). createOverlay.ts:64 calls hideOthers(panel) for modal. substrate.test.tsx asserts z-toast > z-modal. No test asserts the inert behaviour (grep 'inert' across *.test.tsx returns nothing), so the contradiction ships green. Severity medium is right.

### 2. 🟡 computePosition returns maxBlockSize but no consumer applies it — long menus/popovers overflow the viewport with no scroll

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** accessibility
- **Location:** `frontend/packages/design-system/src/overlays/primitives/position.ts:44`

**What:** computePosition returns { left, top, maxBlockSize, placement } (position.ts:37-59), but Menu (Menu.tsx createEffect setPos only reads {left, top}), Popover (Popover.tsx:59-63) and Tooltip (Tooltip.tsx createEffect) all consume only left/top; their floating panels are position:fixed with no max-block-size and no overflow (Menu panel style, Popover panel style, Tooltip tip style all confirmed). So when content exceeds the space below/above the anchor, the overflowing rows render off-screen with no scroll container. Keyboard roving still moves focus to those items (Menu moveActive/itemEls[i].focus), but a fixed element does not scroll into view, so they are visually unreachable.

**Impact:** A row-actions Menu or filter Popover with many entries on a short viewport renders items below the fold that pointer users cannot see/click and keyboard users focus invisibly.

**Fix:** Apply the already-computed clamp: set max-block-size to the returned maxBlockSize and overflow-y:auto on the Menu/Popover/Tooltip panels (and consume `placement` for the flipped case). Add a test with a tall menu asserting a bounded max-block-size and a scrollable region.

> _Verifier note:_ Confirmed: maxBlockSize is computed at position.ts:52 and returned but never read by any of the three consumers; their panel styles have no max-block-size/overflow. Note position.ts:5-10 explicitly documents the helper as BOUNDED and defers 'production polish on tight viewports' — so this is a partly-acknowledged gap. But the value IS computed and simply dropped, which reads as an oversight rather than intentional, and the failure mode (invisible focused items) is real. Dialog itself is unaffected (it hardcodes max-block-size + overflow:auto). Medium is fair.

### 3. 🔵 Reactive dismiss accessor re-runs createOverlay mid-open, churning scroll-lock/inert and yanking focus back to the initial target

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** concurrency
- **Location:** `frontend/packages/design-system/src/overlays/primitives/createOverlay.ts:53`

**What:** resolveBool() is invoked inside the createEffect body (createOverlay.ts:53-54) and calls the accessor forms of closeOnEscape/closeOnOutsidePointer. Dialog passes `closeOnEscape: () => local.dismissable` and `closeOnOutsidePointer: () => local.dismissable` (Dialog.tsx), so the effect subscribes to `dismissable`. If dismissable flips while the dialog is open, the whole effect re-runs: the prior run's onCleanup fires (remove listeners, removeOverlay, unhide, unlockScroll, restore focus) and the body re-runs (pushOverlay, lockScroll, hideOthers, re-fire initial focus).

**Impact:** Toggling a reactive dismiss flag while open (a real pattern: disable dismiss during an in-flight async confirm, then re-enable) re-fires the initial-focus logic — yanking focus back to the initialFocus target (for a plain Dialog, the Close button per finding below) mid-interaction — and churns the refcounted scroll-lock (unlock->relock, restoring/re-adding padding) and the inert background. NOTE: the reviewer's stronger claim — that return-focus on final close is corrupted because previouslyFocused (line 58) is re-captured pointing inside the panel — does NOT hold: Solid runs the previous run's onCleanup (which restores focus to the trigger, createOverlay.ts:118-121) BEFORE the body re-captures previouslyFocused, so previouslyFocused re-captures the trigger, not a panel element, and final close still returns focus correctly.

**Fix:** Read the reactive dismiss flags at event time inside the keydown/pointerdown handlers rather than snapshotting them in the effect body, so toggling them does not re-run the whole setup/teardown. Add a test that toggles dismissable while open and asserts focus, scroll-lock and inert are unaffected.

> _Verifier note:_ Verified the effect DOES re-subscribe: resolveBool(opts.closeOnEscape,...) calls v() at createOverlay.ts:53, and Dialog supplies accessors reading local.dismissable — so re-run on toggle is confirmed, and the scroll/inert/initial-focus churn is a real defect with the right fix. But I traced the focus path: cleanup (line 121) restores focus to (trigger ?? previouslyFocused) BEFORE the body re-runs and re-reads document.activeElement at line 58, so the 'return-focus corrupted into unmounted panel' impact is inaccurate — Dialog uses restoreFocus default true and no triggerRef, so previouslyFocused resolves to the original trigger both times. Severity lowered medium->low: the surviving harm is a focus flicker/yank plus lock churn, not a broken return-focus guarantee.

### 4. 🔵 Menu disabled items use native `disabled`, removing them from menu navigation (APG deviation)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** accessibility
- **Location:** `frontend/packages/design-system/src/overlays/Menu.tsx:234`

**What:** menuitem buttons set the native `disabled` attribute (Menu.tsx:234) in addition to aria-disabled (line 235). Native disabled makes the element unfocusable and drops it from AT navigation entirely, whereas the APG menu pattern keeps disabled items focusable via aria-disabled so their existence/label is perceivable. enabledIndexes() already excludes them from roving/type-ahead, and onClick early-returns on item.disabled, so the native disabled attribute adds nothing but the a11y regression.

**Impact:** Screen-reader users cannot perceive that a disabled action exists (e.g. a greyed 'Delete' hinting a permission), and the announced item count differs from the visual list.

**Fix:** Drop the native `disabled` attribute; rely on aria-disabled + the existing enabledIndexes() skip and onClick early-return. Add a Menu test with a disabled item asserting it is present in the a11y tree but not activatable.

> _Verifier note:_ Confirmed at Menu.tsx:234 (disabled={item.disabled}) alongside aria-disabled and the enabledIndexes()/onClick guards. Real but low-frequency: Menu's own header comment states disabled verbs should be OMITTED entirely ('Items the viewer can't run are simply ABSENT ... never grey-tease'), so in the intended usage disabled items don't render at all — the API's support for item.disabled is itself the inconsistency. No test exercises a disabled item (Menu.test.tsx uses only enabled items). Low is appropriate.

### 5. 🔵 Dialog default initial focus lands on the Close (X) button

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** ux-flow
- **Location:** `frontend/packages/design-system/src/overlays/primitives/createOverlay.ts:72`

**What:** When no initialFocus/autoFocus getter is supplied, createOverlay's fallback querySelector (createOverlay.ts:70-76) picks the first focusable descendant. For Dialog the header (with the Close X button) is rendered before the body and footer (Dialog.tsx header block precedes the body div), so the Close button is the first focusable and receives initial focus. APG permits focusing the first focusable or the dialog itself, but defaulting to the dismiss control means a keyboard user pressing Enter/Space immediately closes the dialog.

**Impact:** Every plain Dialog opened without an explicit initialFocus focuses its Close button, so an immediate Enter/Space closes the dialog instead of engaging the primary content.

**Fix:** Prefer the first focusable in the body, or the panel container (tabindex=-1), over the header dismiss control as the default — e.g. render the close button last in DOM order, or have Dialog pass an initialFocus that targets the body. Add a test asserting the default focus target is not the Close button.

> _Verifier note:_ Confirmed by code + DOM order: fallback selector at createOverlay.ts:72 matches 'button:not([disabled])' first-in-DOM; Dialog.tsx renders the Close button inside the header, which precedes the body/footer, and the Close button only exists when dismissable (default true). ConfirmDialog is unaffected (it passes initialFocus to the safe action). Dialog.test.tsx never asserts the default focus target, so this is unverified by the suite. Low is reasonable (technically APG-compliant, but a UX papercut affecting every default Dialog).

### 6. 🔵 Substrate behaviors exercised by design but not asserted by any test

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** test-coverage
- **Location:** `frontend/packages/design-system/src/overlays/substrate.test.tsx:54`

**What:** Several substrate guarantees have no assertion anywhere in the suite: (1) hideOthers actually marking background children [inert] and restoring them — the inert path (overlay-core.ts:103) is never checked, which is why the inert-toast bug slips through; (2) Escape acting on the topmost overlay only for a real nested Confirm-over-Dialog — the nested test at substrate.test.tsx:54 asserts overlayDepth()===2 but never that Escape dismisses only the Confirm; (3) Menu Tab-closes-and-returns-focus (Menu.tsx onMenuKeydown 'Tab' case); (4) Toast pause-on-hover/pause-on-focus re-arming; (5) scrollbar-width padding compensation in lockScroll (overlay-core.ts:78-82).

**Impact:** Regressions in inert background, topmost-only Escape dismissal, Menu Tab handling, Toast pause, or padding compensation would ship green. The two medium findings above live precisely in these untested seams.

**Fix:** Add assertions for background inert set/restore, nested-Escape topmost-only dismissal, Menu Tab close, Toast pause-on-hover behavior, and padding-right compensation.

> _Verifier note:_ Verified each claimed gap against the test files: grep for 'inert'/'padding'/'paddingRight' across *.test.tsx returns nothing; substrate.test.tsx nested test asserts only overlayDepth (line ~54), not selective Escape; Menu.test.tsx has no 'Tab' case (only Dialog.test.tsx uses Tab, for the trap); Toast.test.tsx tests auto-dismiss + persistent but no pause-on-hover re-arm. All five gaps are real. One correction to the finding's framing: the positive coverage it credits to substrate.test.tsx (trap wrap, Escape+return-focus, scroll-lock apply/restore, roving, type-ahead) actually lives in Dialog.test.tsx / Menu.test.tsx / Toast.test.tsx, not substrate.test.tsx — but the enumerated gaps are accurate. Low is appropriate.

### 7. ⚪ Popover hover variant advertises role=dialog / aria-haspopup=dialog for a hovercard

- **Severity:** nit  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** consistency
- **Location:** `frontend/packages/design-system/src/overlays/Popover.tsx:93`

**What:** The Popover trigger unconditionally sets aria-haspopup="dialog" (Popover.tsx:93) and the floating panel unconditionally uses role="dialog" (Popover.tsx:119) for both the click and hover variants. A hovercard (variant="hover") is a non-modal, non-focus-trapping informational surface; labelling it a dialog with haspopup=dialog over-promises modal-like semantics to AT. axe passes because it is technically valid markup, so the tests do not catch the mismatch.

**Impact:** Assistive tech announces a hovercard as an interactive dialog popup, mis-setting user expectation; minor and non-blocking.

**Fix:** For the hover variant, drop aria-haspopup and use a lighter role (e.g. role=note/group with aria-label) rather than role=dialog; reserve dialog semantics for the interactive click variant.

> _Verifier note:_ Confirmed: aria-haspopup="dialog" at Popover.tsx:93 and role="dialog" at :119 are both unconditional, applied regardless of local.variant. The hover variant deliberately does not trap focus or move focus in (autoFocus getter returns undefined for hover), consistent with a non-modal hovercard, which makes the dialog semantics an overstatement. Nit is correct.
