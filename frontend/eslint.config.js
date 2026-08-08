// Shared Solid reactivity, TypeScript, and accessibility checks.

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
    // Treat broken reactive reads as errors because they fail silently at runtime.
    files: ["**/*.{ts,tsx}"],
    ...solid,
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    rules: {
      ...solid.rules,
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
