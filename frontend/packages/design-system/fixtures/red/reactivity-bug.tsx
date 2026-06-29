// RED FIXTURE — deliberately broken. NOT part of the build or the `lint` script.
// `pnpm lint:prove` lints THIS file and is EXPECTED to fail (the gate proving it bites).
//
// Bug 1 (Solid reactivity): destructuring `props` severs fine-grained reactivity — `label` is read
//   once at call time and never tracks updates. eslint-plugin-solid (solid/reactivity +
//   solid/no-destructure) flags it as an ERROR.
// Bug 2 (a11y): an <img> with no alt text — eslint-plugin-jsx-a11y (jsx-a11y/alt-text) flags it as
//   an ERROR.
import type { JSX } from "solid-js";

export function Broken(props: { label: string; icon: string; onTap: () => void }): JSX.Element {
  const { label, onTap } = props; // solid/no-destructure + solid/reactivity — foot-gun
  return (
    <button onClick={onTap}>
      {/* alt missing — jsx-a11y/alt-text */}
      <img src={props.icon} />
      {label}
    </button>
  );
}
