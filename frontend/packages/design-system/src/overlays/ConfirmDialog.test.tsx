// ConfirmDialog gate: APG alertdialog ARIA, the binding SAFE-action default-focus rule, Escape =
// cancel, and the destructive glyph+label (never colour alone).
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { createSignal } from "solid-js";
import { describe, it, expect, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

function Harness(props: { variant?: "confirm" | "destructive"; onConfirm: () => void; onCancel: () => void }) {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <button onClick={() => setOpen(true)}>Delete repo</button>
      <ConfirmDialog
        open={open()}
        variant={props.variant}
        title="Delete this repository?"
        description="This permanently erases the repository and all its history."
        confirmLabel="Delete"
        onConfirm={() => {
          props.onConfirm();
          setOpen(false);
        }}
        onCancel={() => {
          props.onCancel();
          setOpen(false);
        }}
      />
    </>
  );
}

const open = () => {
  const t = screen.getByRole("button", { name: "Delete repo" });
  t.focus();
  fireEvent.click(t);
  return t;
};

describe("ConfirmDialog", () => {
  it("is axe-clean as an alertdialog with a describedby consequence", async () => {
    render(() => <Harness onConfirm={() => {}} onCancel={() => {}} />);
    open();
    const dlg = screen.getByRole("alertdialog");
    expect(dlg).toHaveAccessibleName("Delete this repository?");
    expect(dlg).toHaveAccessibleDescription(/permanently erases/i);
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("default-focuses the SAFE action (Cancel), never the destructive one", () => {
    render(() => <Harness variant="destructive" onConfirm={() => {}} onCancel={() => {}} />);
    open();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }));
  });

  it("Escape cancels (the safe path) and returns focus to the trigger", () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(() => <Harness variant="destructive" onConfirm={onConfirm} onCancel={onCancel} />);
    const trigger = open();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onConfirm).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
  });

  it("activates the confirm action when clicked", () => {
    const onConfirm = vi.fn();
    render(() => <Harness variant="destructive" onConfirm={onConfirm} onCancel={() => {}} />);
    open();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });
});
