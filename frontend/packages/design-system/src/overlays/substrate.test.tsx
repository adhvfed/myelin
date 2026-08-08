// Substrate gate: (1) the ONE z-index token scale orders the layers correctly (chrome < popover <
// modal < toast) AND each overlay paints with its scale token (never a magic number); (2) the
// gate BITES — a dialog missing its accessible name is caught by axe (proving the a11y gate is real,
// not decorative); (3) nested overlays stack (Confirm-over-Dialog) and the stack tracks depth.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { createSignal } from "solid-js";
import { describe, it, expect } from "vitest";
import { Dialog } from "./Dialog";
import { ConfirmDialog } from "./ConfirmDialog";
import { Menu } from "./Menu";
import { ToastProvider, useToast } from "./Toast";
import { Z_INDEX } from "../../generated/tokens";
import { overlayDepth } from "./primitives/overlay-core";

describe("z-index token scale", () => {
  it("orders the layers chrome < popover < modal < toast (the single scale)", () => {
    expect(Z_INDEX.chrome).toBeLessThan(Z_INDEX.popover);
    expect(Z_INDEX.popover).toBeLessThan(Z_INDEX.modal);
    expect(Z_INDEX.modal).toBeLessThan(Z_INDEX.toast);
  });

  it("each overlay paints with its scale TOKEN, never an inline magic number", () => {
    function Both() {
      const toast = useToast();
      return (
        <>
          <button onClick={() => toast.show({ title: "hi" })}>Toast</button>
          <Dialog open={true} onClose={() => {}} title="Modal">
            body
          </Dialog>
        </>
      );
    }
    render(() => (
      <ToastProvider>
        <Both />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Toast" }));
    // The modal layer is the dialog's positioned ancestor (the fixed full-viewport wrapper).
    const modalLayer = screen.getByRole("dialog").parentElement!;
    expect(modalLayer.style.zIndex).toBe("var(--z-modal)");
    expect(screen.getByRole("region", { name: "Notifications" }).style.zIndex).toBe("var(--z-toast)");
  });

  it("Menu paints on the popover layer token", () => {
    render(() => <Menu label="m" items={[{ label: "A", onSelect: () => {} }]} triggerLabel="Open" />);
    fireEvent.click(screen.getByRole("button", { name: /Open/ }));
    expect(screen.getByRole("menu").style.zIndex).toBe("var(--z-popover)");
  });
});

describe("nested overlays (Confirm over Dialog)", () => {
  it("stacks both modals; the overlay depth reflects the stack", () => {
    function Nest() {
      const [confirm, setConfirm] = createSignal(false);
      return (
        <Dialog open={true} onClose={() => {}} title="Outer">
          <button onClick={() => setConfirm(true)}>Danger</button>
          <ConfirmDialog
            open={confirm()}
            title="Sure?"
            description="This is irreversible."
            onConfirm={() => setConfirm(false)}
            onCancel={() => setConfirm(false)}
          />
        </Dialog>
      );
    }
    render(() => <Nest />);
    expect(overlayDepth()).toBe(1);
    fireEvent.click(screen.getByRole("button", { name: "Danger" }));
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    expect(overlayDepth()).toBe(2); // both layers are live on the stack
  });
});

describe("the gate BITES (negative / red check)", () => {
  it("flags a dialog that is missing its accessible name", async () => {
    // A deliberately broken overlay — role=dialog + aria-modal but NO label. The substrate forbids
    // this by construction (Dialog wires aria-labelledby); here we prove axe catches the violation
    // if a hand-rolled overlay skips it.
    render(() => (
      <div role="dialog" aria-modal="true">
        <button>OK</button>
      </div>
    ));
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    const violationIds = results.violations.map((v) => v.id);
    expect(results.violations.length).toBeGreaterThan(0);
    expect(violationIds).toContain("aria-dialog-name");
  });
});
