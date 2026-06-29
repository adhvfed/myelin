import "@testing-library/jest-dom/vitest";
// Type augmentation for the axe matcher (toHaveNoViolations) on vitest's expect.
import "vitest-axe/extend-expect";
import { expect } from "vitest";
import * as matchers from "vitest-axe/matchers";

// Register the axe matcher at runtime.
expect.extend(matchers);
