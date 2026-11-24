import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CONFIRM_DIALOG_CSS, ConfirmDialog } from "./ConfirmDialog";

describe("ConfirmDialog server rendering", () => {
  const props = {
    open: true,
    title: "Discard changes?",
    description: "Unsaved edits will be lost.",
    confirmLabel: "Discard",
    onConfirm: () => undefined,
    onCancel: () => undefined,
  };

  it("renders a labelled alertdialog with the safe action first", () => {
    const html = renderToStaticMarkup(createElement(ConfirmDialog, props));
    expect(html).toContain('role="alertdialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toMatch(/aria-labelledby="([^"]+)-title"/);
    expect(html).toMatch(/id="([^"]+)-title"[^>]*>Discard changes\?/);
    expect(html).toMatch(/aria-describedby="([^"]+)-description"/);
    expect(html).toMatch(/id="([^"]+)-description"[^>]*>Unsaved edits will be lost\./);
    expect(html.indexOf(">Cancel</button>"))
      .toBeLessThan(html.indexOf(">Discard</button>"));
    expect(renderToStaticMarkup(createElement(ConfirmDialog, { ...props, open: false }))).toBe("");
  });

  it("ships only app tokens and a fixed in-app modal treatment", () => {
    expect(CONFIRM_DIALOG_CSS).toContain("position:fixed");
    expect(CONFIRM_DIALOG_CSS).toContain("var(--widget-bg)");
    expect(CONFIRM_DIALOG_CSS).toContain("var(--overlay-shadow)");
    expect(CONFIRM_DIALOG_CSS).toContain("var(--danger-bg)");
    expect(CONFIRM_DIALOG_CSS).toContain("var(--danger-fg)");
    expect(CONFIRM_DIALOG_CSS).toContain("var(--font-ui)");
    expect(CONFIRM_DIALOG_CSS).toContain("min-height:28px");
    expect(CONFIRM_DIALOG_CSS).toContain(":not(:disabled):hover");
    expect(CONFIRM_DIALOG_CSS).not.toContain("linear-gradient");
  });

  it("turns an accepted handoff into a non-dismissible busy status", () => {
    const html = renderToStaticMarkup(createElement(ConfirmDialog, {
      ...props,
      pending: true,
      title: "Preparing the update",
      description: "Interface will restart automatically.",
    }));
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('role="status"');
    expect(html).toContain("Keep Interface open while macOS prepares the restart.");
    expect(html).not.toContain("<button");
  });

  it("keeps Cancel before Confirm and uses the shared modal lifecycle", () => {
    const source = ConfirmDialog.toString();
    expect(source.indexOf("cancelRef")).toBeLessThan(source.indexOf("confirmRef"));
    expect(source).toContain("useModalFocus");
    expect(source).toContain("initialFocus");
    expect(source).toContain("returnFocusRef");
  });
});
