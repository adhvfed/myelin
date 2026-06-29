// Augment vitest's expect with the axe matcher type (runtime registration is in vitest.setup.ts).
import type { AxeMatchers } from "vitest-axe";

declare module "vitest" {
  interface Assertion<T = any> extends AxeMatchers {}
  interface AsymmetricMatchersContaining extends AxeMatchers {}
}
