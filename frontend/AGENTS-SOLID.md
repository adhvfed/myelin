# Solid patterns for agents

The frontend analog of the design manual: the concrete, copy-pasteable patterns an agent uses to
build **correct** SolidJS components for Myelin. Read this **and**
`planning/system-reviews/2026-06-26/10-frontend-component-patterns.md` before writing any UI.

- **This doc** = Solid fluency: reactivity rules, the primitive pattern *shapes*, data, a11y, tokens,
  the foot-guns. It is the mitigation for Solid's lower agent-training fluency (doc 08 §5).
- **Doc 10** = the per-component *behavioural* approach (overlays, command palette, the per-block
  editor, the data gateway, SSE, the app shell). The hard parts, already solved.
- **The design manual** (`design-planning/08-design-system/`) = the look (Tier-0 tokens) + the a11y
  spec (WCAG 2.2 AA / WAI-ARIA APG). PROVEN/binding; meet it, don't import it.

The lint (`frontend/eslint.config.js`: `eslint-plugin-solid` + `eslint-plugin-jsx-a11y`) and the axe
test are the gate. If it's red, the component is wrong — fix the component, never the gate.

---

## 0. Dependency stance: minimal, hand-built

`solid-js` + SolidStart (`@solidjs/router`, `@solidjs/meta`) + the `myelin-content` WASM parse/
serialize layer. **Hand-build the primitives.** A headless a11y lib (Kobalte), an editor framework
(ProseMirror), or a query lib (TanStack) are options of **last resort** for one component that proves
intractable — never the default. The a11y *bar* is non-negotiable; we meet it ourselves and gate it
with axe + keyboard tests.

---

## 1. Reactivity — the rules that prevent the silent-failure class

Solid is fine-grained: a component function runs **once**; reactivity lives in the values you *read*
inside tracking scopes (JSX, `createEffect`, `createMemo`). Break tracking and the UI silently stops
updating. These rules are enforced by `eslint-plugin-solid`.

### 1.1 NEVER destructure `props` (or `store`)

```tsx
// WRONG — `label` is read once; it never updates. (solid/no-destructure)
function Badge({ label }: { label: string }) { return <span>{label}</span>; }

// RIGHT — read at the use-site so the access is tracked.
function Badge(props: { label: string }) { return <span>{props.label}</span>; }
```

Need defaults / to forward the rest? Use `mergeProps` + `splitProps` (see `src/Icon.tsx`):

```tsx
const merged = mergeProps({ size: 16 }, props);
const [local, rest] = splitProps(merged, ["name", "size"]);
// local.size is reactive; spread {...rest} onto the element.
```

### 1.2 Read reactive values at the use-site, inside tracking scope

```tsx
const [count, setCount] = createSignal(0);

// WRONG — pulls the value out of tracking once.
const c = count();
return <p>{c}</p>;

// RIGHT — call the accessor in the JSX (tracked).
return <p>{count()}</p>;
```

Pass *accessors/functions* down, not snapshot values, when a child must stay reactive.

### 1.3 Derive with `createMemo`; run side effects with `createEffect`

```tsx
const fullName = createMemo(() => `${first()} ${last()}`); // cached derived value
createEffect(() => { document.title = fullName(); });       // side effect on change
onCleanup(() => { /* unsubscribe, clearTimeout, removeEventListener */ });
```

`createEffect` runs **after** render and re-runs when its tracked reads change. Put teardown in
`onCleanup` (it runs before the next effect run and on unmount).

### 1.4 Control flow: components, not ternaries / `.map`

```tsx
<Show when={user()} fallback={<LoginPrompt />}>{(u) => <Profile user={u()} />}</Show>
<For each={items()}>{(item) => <Row item={item} />}</For>      // keyed by reference; use For, not .map
<Index each={items()}>{(item, i) => <Cell value={item()} />}</Index> // keyed by index (primitive lists)
<Switch fallback={<Empty />}>
  <Match when={state() === "loading"}><Spinner /></Match>
  <Match when={state() === "error"}><ErrorState /></Match>
</Switch>
```

`<For>` re-uses DOM nodes by item identity (efficient lists); `.map()` rebuilds everything and loses
state. `eslint-plugin-solid` flags `.map` in JSX.

### 1.5 Nested / structured state: stores, not signals-of-objects

```tsx
const [state, setState] = createStore({ rows: [] as Row[], selected: new Set<string>() });
setState("rows", (r) => [...r, newRow]);        // path-based, fine-grained updates
setState("rows", (r) => r.id === id, "done", true);
```

A store gives per-property reactivity; a `createSignal({...})` re-runs everything on any change.

### 1.6 The reactivity pitfalls checklist (the foot-guns)

- Destructuring `props`/stores (1.1) — the #1 agent mistake.
- Reading a signal once into a `const` and rendering the const (1.2).
- Using `.map` instead of `<For>`; ternary chains instead of `<Show>/<Switch>`.
- A signal-of-object where a store is needed (1.5).
- Conditionals that early-`return` different JSX from the component body (the body runs once — branch
  *inside* the returned JSX with `<Show>`).
- Forgetting `onCleanup` for listeners/timers/subscriptions → leaks.
- Async work in `createEffect` without guarding against stale runs — prefer `createResource`.

---

## 2. The hand-built overlay primitive (pattern shape — full build is MR-017)

Every modal-class surface is ONE `Dialog` primitive (doc 10 §1). The non-negotiable mechanics:

- **Portal to `document.body`** (`<Portal>`) — escapes transformed/clipped ancestors (the app
  shell's nav aside carries a `transform`; a non-portaled `position:fixed` panel mis-anchors).
- **Body scroll-lock with scrollbar-width compensation** (no sideways jolt on open).
- **A real focus trap** — focus moves in on open; Tab/Shift+Tab wrap inside; focus returns to the
  trigger on close.
- **Escape + backdrop dismiss**, each independently disableable; the Escape handler
  `stopPropagation`s so a modal opened inside a panel doesn't also collapse the panel.
- `role="dialog"` + `aria-modal` + `aria-labelledby`/`aria-describedby`.
- A **custom-header slot** (so the command palette is `Dialog` + a search input, not a new overlay).

```tsx
import { Portal } from "solid-js/web";
function Dialog(props: { open: boolean; onClose: () => void; children: JSX.Element }) {
  let panel!: HTMLDivElement;
  createEffect(() => {
    if (!props.open) return;
    const prev = document.activeElement as HTMLElement | null;
    lockScroll();
    panel.querySelector<HTMLElement>("[autofocus],button,[href],input,select,textarea")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.stopPropagation(); props.onClose(); }
      if (e.key === "Tab") trapFocus(e, panel);
    };
    document.addEventListener("keydown", onKey, true);
    onCleanup(() => { document.removeEventListener("keydown", onKey, true); unlockScroll(); prev?.focus(); });
  });
  return (
    <Show when={props.open}>
      <Portal>
        <div role="presentation" onClick={props.onClose} style={{ "z-index": "var(--z-modal)" }}>
          <div ref={panel} role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
            {props.children}
          </div>
        </div>
      </Portal>
    </Show>
  );
}
```

The six primitives built once and inherited: **Dialog · ConfirmDialog** (`alertdialog`, default focus
on the *safe* action; irreversible/GDPR/HITL) **· Popover · Dropdown/Menu** (roving) **· Tooltip**
(never takes focus; hover *and* focus) **· Toast** (never steals focus; AT via live region). One
z-index scale: `chrome < popover < modal < toast` (`Z_INDEX` in tokens / `--z-*` vars).

One shared **viewport-clamp helper** positions every caret/anchor float (mention picker, slash menu,
reaction picker, block-handle menu) — written once, never copy-pasted (doc 10 §2).

---

## 3. Data layer (SolidStart-native + a server-side gateway client)

```tsx
// server-side ONLY — runs in loaders / server functions / API routes. Tokens never reach client JS.
const data = createAsync(() => getThings());           // SolidStart loader / createAsync
const [resource] = createResource(source, fetcher);    // imperative fetch with loading/error
```

- **One server-side gateway client** handles every backend call: reads the session from an httpOnly
  cookie, adds the Bearer token, on 401 does a single refresh + one retry, else throws
  `Unauthorized` (the loader turns it into a `/login` redirect). **Tokens never reach client JS** —
  this is why SSR earns its keep (doc 10 §5).
- **Typed errors:** `Unauthorized` and `GatewayError` (extracts the `{error:{message}}` envelope so
  toasts read like the API author wrote them; preserves status + raw body).
- **Real-time:** `EventSource` (SSE), proxied by a SolidStart API route (doc 10 §6). No client WS.
- No third-party query library — the router's data layer handles cache/dedupe.

---

## 4. Tokens & icons (the ONLY styling vocabulary)

- **Components read SEMANTIC CSS vars only.** Never a primitive var, never a hex, in markup:
  `color: var(--text-primary)`, `background: var(--surface-raised)`, `border: var(--hairline) solid
  var(--border)`. The 3 themes (`data-theme="dark|light|high-contrast"`) re-skin by re-pointing the
  same semantic vars — your component needs zero theme branches.
- `tokens.css` + the TS constants are **generated** by Style Dictionary from the canonical DTCG
  `design-planning/08-design-system/01-tokens/tokens.json`. Never hand-edit generated output; never
  fork token values. Re-skin = edit the `$themes` map in tokens.json and rebuild.
- Reach for the TS constants only where CSS vars can't: `Z_INDEX.modal` for imperative stacking,
  `themeColors` for canvas/chart fills or `<meta name="theme-color">`.
- **Icons:** `<Icon name="..." />` over the self-hosted sprite (no CDN). Decorative by default
  (`aria-hidden`); pass `title` only when the icon is the *sole* carrier of meaning. Status is
  **never** color-only — always glyph **+** text label (WCAG 1.4.1). The name is typed against the
  42-icon set (`IconName`).

---

## 5. Accessibility — the gate, not an afterthought

- Every interactive element is reachable and operable by keyboard; visible focus comes free from the
  one `:focus-visible` rule in `tokens.css` (don't remove outlines).
- Use semantic elements (`<button>`, `<a href>`, `<label>`) before ARIA. Match the component spec's
  WAI-ARIA APG roles/keys exactly (`design-planning/08-design-system/02-components/*.md`).
- Every component gets an **axe test** (see `styleguide/Demo.test.tsx`) and, for interactive ones, a
  **keyboard test**. `jsx-a11y` catches the static violations at lint time; axe catches the rest.
- Unglamorous states are first-class: empty / loading / error / permission-denied / erased-tombstone
  / agent-pending. Build them; they're in the specs.

---

## 6. The spec → component convention

Each `design-planning/08-design-system/02-components/<x>.md` spec → **one** hand-built Solid
component: ALL states implemented, tokens-only, with its axe + keyboard test. No subsystem ships its
own primitive — a missing primitive is contributed *down* into the shared package, never forked up.

---

## 7. Testing

- Component smoke + a11y: `vitest` + `@solidjs/testing-library` + `vitest-axe` (jsdom). Pattern in
  `styleguide/Demo.test.tsx`.
- Real-browser e2e (later): Playwright against the running SolidStart app + `@axe-core/playwright`.
  The "switch test" becomes a real browser-driven test (doc 08 §8).
- The block editor round-trip gate `render(parse(md)) === md` is reused from `myelin-content`.
