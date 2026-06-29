# Red fixtures — proving the lint bites

"A gate must prove it bites." These files contain deliberate violations the frontend lint MUST
reject. They are excluded from the package `lint` script (which lints only real source and stays
green) and from `tsconfig`/`vitest`.

Run the proof:

```sh
pnpm --filter @myelin/design-system lint:prove   # EXPECTED: non-zero exit, errors listed
```

`reactivity-bug.tsx` trips two rule classes at once:

- **Solid reactivity** — `solid/no-destructure`: destructuring `props` severs fine-grained
  reactivity (the silent-at-runtime failure class this whole gate exists to catch).
- **Accessibility** — `jsx-a11y/alt-text`: an `<img>` with no `alt`.

If `lint:prove` ever exits 0, the gate has stopped biting — treat that as a build break.
