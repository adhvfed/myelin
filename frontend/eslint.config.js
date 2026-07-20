// Myelin frontend lint — the agent-fluency gate (frontend canon doc 08 §5.2 / doc 10 §0).
//
// This is the clippy-equivalent for the Solid frontend. It exists to catch the two failure
// classes that an LLM agent most often introduces when hand-writing Solid:
//   1. Solid REACTIVITY foot-guns (eslint-plugin-solid) — destructured props, reactive reads
//      pulled out of tracking scope, missing <Show>/<For>, etc. These break fine-grained
//      reactivity SILENTLY at runtime; the lint makes them loud at author time.
//   2. ACCESSIBILITY violations (eslint-plugin-jsx-a11y) — the a11y bar is PROVEN/binding in the
//      design manual; we meet it ourselves and gate it (we do NOT import a11y from a component lib).
//
// "A gate must prove it bites": fixtures/red/ holds deliberate violations of BOTH classes. The
// package `lint` script lints only real source (green); `lint:prove` lints the red fixtures and is
// EXPECTED to exit non-zero. See packages/design-system/fixtures/red/README.md.

import js from "@eslint/js";
import tseslint from "typescript-eslint";
import solid from "eslint-plugin-solid/configs/typescript";
import jsxA11y from "eslint-plugin-jsx-a11y";

export default tseslint.config(
  {
    // Never lint dependencies, build output, or generated tokens.
    ignores: ["**/node_modules/**", "**/dist/**", "**/generated/**"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // Solid reactivity rules — the core agent-fluency mitigation.
    files: ["**/*.{ts,tsx}"],
    ...solid,
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    rules: {
      ...solid.rules,
      // A gate is red-on-violation: the reactivity foot-guns (the silent-at-runtime failure class
      // this whole lint exists to catch) are ERRORS, not warnings.
      "solid/reactivity": "error",
      "solid/no-destructure": "error",
    },
  },
  {
    // Accessibility rules on every JSX surface.
    files: ["**/*.{tsx,jsx}"],
    plugins: { "jsx-a11y": jsxA11y },
    rules: {
      ...jsxA11y.flatConfigs.recommended.rules,
      // `list-style: none` can erase native list semantics in Safari/VoiceOver. Explicit
      // `role="list"` restores that contract and is intentionally not redundant there.
      "jsx-a11y/no-redundant-roles": ["error", { nav: ["navigation"], ul: ["list"] }],
    },
  },
  {
    // Config / build / test tooling runs in Node; provide Node globals + relax a couple of rules.
    files: ["**/*.{config,test,spec}.{ts,tsx,mts,mjs}", "**/scripts/**", "**/*.mjs"],
    languageOptions: {
      globals: {
        console: "readonly",
        process: "readonly",
        URL: "readonly",
        document: "readonly",
      },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
      "no-undef": "off",
    },
  },
  {
    // Ambient type-augmentation files: empty-interface merging + the generic mirror are intentional.
    files: ["**/*.d.ts"],
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-unused-vars": "off",
      "@typescript-eslint/no-empty-object-type": "off",
    },
  },
);
