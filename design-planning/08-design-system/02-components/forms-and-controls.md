# Forms & Controls — Button · Input · Select/Combobox · Checkbox/Radio/Switch · Field + Validation

> **Tier 2 (the form atoms).** The interactive control set every surface composes from — the atoms inside
> dialogs, the HITL Edit form, views inline-edit, the comment composer, and field-definition UI ([R]
> consumers). **File date: 2026-06-20. Direction A "Instrument".**
>
> **Implements:** design-language §5 (shared chrome) + §8b.3 (measured tokens / focus≠identity / status-not-
> colour / never inline-colour-on-interactive / hierarchy by weight & colour) + R-17 §4 (the G1 master
> checklist — labels, focus, keyboard, status-not-colour). **Maps to React Aria Components** per control.
>
> **Tagging:** **PROVEN** = WCAG 2.2 / WAI-ARIA APG / the focus-token derivation rule. **HOUSE STYLE** =
> direction-A character (28px compact controls, hairlines, near-zero radius).

---

## 1. Shared control rules (apply to ALL controls below) — PROVEN

- **The one derived `focus-ring` token** renders on every interactive element via `:focus-visible` (2px,
  logical offset; tokens.css foundation rule). **`focus-ring` ≠ `accent`** — and the **primary-action fill is
  `--c-btn-primary-bg` → `--focus-ring`, NOT `--accent`** (the carve-out: the light-theme `accent` is exactly
  4.50:1, fine as a brand mark but a fragile button background; the derived token is 6.55:1). *(PROVEN — tokens
  §2.4; the prompt's "primary on the focus/derived token".)*
- **Status never by colour alone** — validation/state carries glyph + label + position (WCAG 1.4.1).
- **Never set colour via inline style on an interactive element** — inline style beats `:hover`/`:focus`
  specificity; colour comes from tokens only (a Phase-8 lint enforces it). *(PROVEN.)*
- **Hierarchy from weight & colour before size; spacing on the 4px ramp** (no 5/7/13px magic numbers).
- **Compact default** (`--control-h:28px`, `--row-h:32px`); `density:comfortable` lifts to 32/36 via tokens.
- **Logical properties throughout** → RTL mirrors for free. **Targets ≥24×24 CSS px** (WCAG 2.5.8) — the 28px
  control height + spacing satisfies this.
- **Every field has a programmatic label** (1.3.1 / 4.1.2) — placeholder is never the label.

---

## 2. Button

**Purpose.** The action control. **Implements** §8b.3 (primary on the derived token) + the finalist-A `.btn`.

**Anatomy.** label (weight per variant) + optional leading/trailing icon (inherits `currentColor`) + optional
in-button spinner (loading). Hairline border + `--radius-1`, height `--control-h`.

**Variants (+ token binding).**

| Variant | Background | Text | Border |
|---|---|---|---|
| **primary** (the ONE primary per surface) | `--c-btn-primary-bg` (→ `--focus-ring`) | `--c-btn-primary-text` (`--on-accent`) | same as bg |
| **secondary / default** | `--surface-raised` / `--surface-overlay` | `--text-primary` | `--border-strong` |
| **ghost / tertiary** | transparent | `--text-muted` | none (hover → `--surface-hover`) |
| **danger** | `--c-btn-danger-bg` (`--danger`) | `--on-danger` | same | (reserved for destructive; pairs with ConfirmDialog) |
| **link** | none | `--focus-ring` (info/link colour) | underline |

**Sizes.** `sm 24 · md 28 (default) · lg 32`. `iconOnly` (square; **requires `aria-label` + a Tooltip**).

**Parameterization variant flags.** `density` sets size; `tone` may affect label copy voice only. No
`switch(direction)`.

**States.** default · **hover** (border → `--text-subtle` / bg → `--surface-hover`) · **focus** (`--focus-ring`)
· **active/pressed** (slight inset) · **disabled** (`--text-subtle`, `aria-disabled`, not a colour-only cue —
also reduced opacity + no pointer) · **loading** (in-button spinner + `aria-busy="true"`, label stays for width
stability, control non-activatable — the optimistic-action affordance, 00-plan §4.2).

**Keyboard + ARIA (PROVEN).** native `<button>` semantics; Enter/Space activate; `aria-disabled` for
soft-disable (keeps it focusable to explain why) or `disabled` for hard. → React Aria **`Button`** /
`ToggleButton`.

**Motion.** press feedback `--dur-micro`; reduced-motion → instant.

**Do / Don't.** **Do** make primary ride `--focus-ring` (never raw `--accent`); **do** one primary per surface;
**do** give icon-only buttons a label + tooltip. **Don't** convey the only meaning of a disabled state by
colour; **don't** inline-style the colour.

---

## 3. Input (text / number / textarea-like single line)

**Purpose.** Single-line text/number entry. **Implements** R-17 §4 (M5 labels) + finalist-A `.input`.

**Anatomy.** field shell (`--surface-sunken`-equivalent → `--surface` with hairline `--border-strong`,
`--radius-1`, height `--control-h`) + value text `--text-primary` + optional leading icon / trailing
affordance (clear, unit) + placeholder `--text-subtle`.

**Variants.** `text · number · search · password · email`; `withIcon`; `withClear`. (Multiline = the [R] block
editor / a textarea variant; the controlled rich editor is [R]'s.)

**Parameterization variant flags.** `density` (height/padding). `tone`/others: none.

**States.** default · **hover** (border → `--text-subtle`) · **focus** (`--focus-ring`; border-strong) ·
**filled** · **disabled** (`--text-subtle`, no caret) · **readonly** · **loading** (async validate → trailing
spinner + `aria-busy`) · **error** (see §6: `aria-invalid` + `--danger` border + glyph + message) · **empty**
(placeholder, not a label substitute).

**Keyboard + ARIA (PROVEN).** native input; programmatic `<label>`/`aria-labelledby`; `aria-describedby` →
help/error text; `aria-invalid` on error. → React Aria **`TextField`** (`Label`/`Input`/`Text`/`FieldError`).

**Tokens.** `--surface`, `--border-strong`, `--text-primary/subtle`, `--focus-ring`, `--danger` (error),
`--radius-1`.

**Do / Don't.** **Do** keep a real `<label>`; **do** wire `aria-describedby` to errors. **Don't** use the
placeholder as the label; **don't** signal error by red border alone (add glyph + message).

---

## 4. Select / Combobox

**Purpose.** Choose from a set; **combobox** when type-to-filter is needed (the common case; mirrors the
palette's combobox pattern at field scale). **Implements** APG combobox + R-08's permission/schema-aware value
lists where used inside the query surface.

**Anatomy.** trigger (looks like an Input + chevron) → [F] **Popover** hosting a `listbox` of options (the
shared row atom: icon + label + optional description + selected check). Multi-select renders chosen values as
removable chips (logical-property-based → RTL-safe).

**Variants.** `select` (no filter, `listbox` popup) · `combobox` (editable, type-to-filter, `aria-
activedescendant`) · `multi` (chips) · `async` (loads options) · `creatable` (add a new value).

**Parameterization variant flags.** `density` (row height); option lists are permission-/schema-aware where
they back a query field (only values the viewer may filter on).

**States.** default · hover · **focus** (`--focus-ring`) · open (popup) · **active option** (roving via
`aria-activedescendant`; `--focus-ring` marker) · selected (check) · disabled · **loading** (`async` → skeleton
rows + `aria-busy` in the popup, never spinner) · **empty/no-match** (quiet "No matches" + create hatch if
`creatable`) · **error** (§6) · **permission-denied** (an option the viewer can't pick is **absent**, not greyed).

**Keyboard + ARIA (PROVEN — APG combobox/listbox).** `role="combobox"`+`listbox`+`option`; DOM focus on the
input (combobox) with `aria-activedescendant`; ↑/↓/Home/End/type-ahead; Enter selects; Esc closes + returns.
Popup inherits the [F] Popover (flip/max-height/return-focus). → React Aria **`ComboBox`** / **`Select`** /
**`ListBox`** (+ `Popover`).

**Tokens.** as Input + `--surface-overlay`/`--shadow-popover` (popup), `--surface-hover` (option hover),
`--accent-weak` (active wash), `--z-popover`.

**Do / Don't.** **Do** prefer combobox (type-to-filter) for >~7 options; **do** auto-flip the popup; **do**
make value lists permission-aware in query contexts. **Don't** reinvent the listbox (use the shared row atom);
**don't** trap focus (the popup is non-modal).

---

## 5. Checkbox / Radio / Switch

**Purpose.** Boolean and single-choice toggles. **Implements** APG checkbox / radio-group / switch.

**Anatomy.** control box/circle/track (`--border-strong`, `--radius-1` for checkbox; circle for radio; pill
track for switch) + label `--text-primary` (clickable) + optional description `--text-subtle`. **Checked fill =
`--c-btn-primary-bg` (→ `--focus-ring`)** with `--on-accent` glyph — not raw `--accent`.

**Variants.** checkbox (`indeterminate` supported) · radio (in a `radiogroup`) · switch (on/off; for immediate
settings, not form submit). sizes `sm 16 · md 18`.

**Parameterization variant flags.** `density` (size + row gap). Others: none.

**States.** unchecked · checked · **indeterminate** (checkbox) · hover · **focus** (`--focus-ring`) · active ·
disabled (`--text-subtle` + non-interactive, with the state still conveyed by shape, not colour-only) ·
**error** (a required-group failure → §6 message on the group). **The checked state is conveyed by the glyph/
position, not colour alone** (1.4.1).

**Keyboard + ARIA (PROVEN).** Space toggles checkbox/switch; radio group is one Tab-stop, arrows move within
(roving); `role="switch"` exposes on/off as `aria-checked`. Labels programmatically associated. → React Aria
**`Checkbox`** / **`RadioGroup`+`Radio`** / **`Switch`**.

**Tokens.** `--border-strong`, `--c-btn-primary-bg` (checked fill), `--on-accent` (glyph), `--text-primary/
subtle`, `--focus-ring`, `--danger` (group error).

**Do / Don't.** **Do** make the label clickable; **do** use a switch only for immediate-effect settings; **do**
support indeterminate where a parent gathers children. **Don't** rely on the fill colour alone for checked;
**don't** use a switch for a form field that needs submit.

---

## 6. Field + Validation (the wrapper)

**Purpose.** The labelled wrapper that gives any control above its **label + help + error** and the validation
contract. **Implements** R-17 §4 (M5 labels, M2 focus) + §8b.6 (error blames the system / preserves input).

**Anatomy.** `<label>` (`--text-primary`, weight-medium) + optional required marker (a `*` **plus** the word
"required" for SR — never `*` alone) + the control + help text (`--text-subtle`, `aria-describedby`) + error
slot (`--danger` text + a danger glyph + the message; `aria-describedby` + `aria-invalid` on the control).

**Variants.** `required` · `optional` · `inline` (label beside) vs `stacked` (label above; default) ·
`groupField` (fieldset/legend for radio/checkbox groups).

**Parameterization variant flags.** `density` (label gap, control height). `tone` shapes help/error voice
(utilitarian default; warm/sober per direction) — copy only, not chrome.

**States (validation).**
- **pristine / valid:** no message.
- **focus:** `--focus-ring` on the control.
- **error (field-level):** `aria-invalid="true"`, danger border + glyph + a **concise, system-/input-blaming
  message** ("Enter a date on or after today"), wired via `aria-describedby`; **input is never cleared.**
- **error (form-level submit):** a summary region (focusable, `role="alert"` on submit-fail) listing fields +
  links; focus moves to the first invalid field.
- **async/server error:** one quiet system-blaming line; **the user's input is preserved** (§8b.6).
- **loading (async validate/submit):** the submit button shows its loading state (§2); `aria-busy`.
- **disabled / readonly:** propagated to the control.

**Keyboard + ARIA (PROVEN).** label↔control association (1.3.1/4.1.2); `aria-describedby` for help+error;
`aria-invalid` for error; groups use `fieldset`/`legend`. **Validation announced via a live region** without
spam — announce on blur/submit, not per keystroke (4.1.3; R-17 §6.1). → React Aria **`TextField`/`Select`/etc.
+ `FieldError`** (built-in `aria-invalid`/`aria-describedby` wiring) inside a `Form`.

**Tokens.** `--text-primary` (label), `--text-subtle` (help), `--danger`/`--danger-subtle` (error, + glyph),
`--focus-ring`, spacing ramp.

**Do / Don't.** **Do** keep a real label + describedby errors; **do** preserve input on error; **do** announce
validation politely on blur/submit. **Don't** mark required with `*` alone; **don't** blank the form on a
server error; **don't** announce per keystroke.

---

## 7. Reuse seam (with the rich-components set)

These atoms are composed by [R]: the **HITL Edit form** (typed against a `ToolDef` JSON Schema — R-14 §3.3),
**views inline-edit** field editors (text/number/select/date/person/relation), the **comment composer**, and
**field-definition UI**. The *cell-editor / field-editor molecules* are [R]'s; they are built from these [F]
atoms and must not re-implement focus/validation/ARIA. The **Select/Combobox** here is the field-scale sibling
of the **command palette's** combobox (§ command-palette.md) — same APG pattern, same row atom.

## 8. Carried flags (honesty)

- The **primary-on-derived-token** rule (§1) is **PROVEN** (the measured carve-out); it is the single most
  important forms decision and is enforced by the contrast CI gate over the generated tokens.
- Whether the validation politeness (blur/submit vs live) is right for real AT users on a busy form is
  **`[DEFERRED-UNTIL-USERS]`** (R-17 §8) — the expert default is blur/submit; confirm with AT-user testing.
