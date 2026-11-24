import { createElement } from "react";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  PROVIDER_COMMAND_MENU_CSS, ProviderCommandMenu, providerCommandMenuAvailableHeight,
  providerCommandDescriptionPreview, providerCommandOptionId,
} from "./ProviderCommandMenu";

describe("ProviderCommandMenu", () => {
  it("renders a provider-labelled listbox with selectable command copy", () => {
    const html = renderToStaticMarkup(createElement(ProviderCommandMenu, {
      id: "commands", providerLabel: "Grok", activeIndex: 0,
      commands: [
        { name: "review", description: "Review changes", argumentHint: "<target>" },
        { name: "compact", description: "Compact context" },
      ],
      onChoose: () => undefined, onActiveIndexChange: () => undefined,
    }));
    expect(html).toContain('role="listbox"');
    expect(html).toContain('aria-label="Grok commands"');
    expect(html).toContain('id="commands-option-0"');
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('tabindex="-1"');
    expect(html).toContain("/review");
    expect(html).toContain("&lt;target&gt;");
    expect(html).toContain("Review changes");
    expect(html).toContain("Enter run");
    expect(html).toContain("Tab insert");
  });

  it("shows a truthful empty result without inventing commands", () => {
    const html = renderToStaticMarkup(createElement(ProviderCommandMenu, {
      id: "commands", providerLabel: "Claude", activeIndex: 0, commands: [],
      onChoose: () => undefined, onActiveIndexChange: () => undefined,
    }));
    expect(html).toContain("No matching commands");
    expect(html).not.toContain('role="option"');
  });

  it("can participate in New Chat layout instead of escaping its clipped card", () => {
    const html = renderToStaticMarkup(createElement(ProviderCommandMenu, {
      id: "commands", providerLabel: "Codex", activeIndex: 0, commands: [],
      placement: "inline", emptyLabel: "Loading Codex commands for gpt-5.6…",
      onChoose: () => undefined, onActiveIndexChange: () => undefined,
    }));
    expect(html).toContain('class="provider-command-menu is-floating is-inline"');
    expect(html).toContain("Loading Codex commands for gpt-5.6…");
    expect(html).not.toContain("↑↓ choose");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain(".provider-command-menu.is-inline");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("max-height:min(248px, 36vh)");
  });

  it("uses the shared overlay and accessible state tokens", () => {
    expect(providerCommandOptionId("menu", 3)).toBe("menu-option-3");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("var(--dropdown-bg)");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("var(--row-selected)");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("var(--accent-text)");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("grid-template-columns:minmax(0, 40%)");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("text-overflow:ellipsis");
    expect(PROVIDER_COMMAND_MENU_CSS).toContain("@container provider-command-menu");
    expect(providerCommandMenuAvailableHeight(280, 80)).toBe(188);
    expect(providerCommandMenuAvailableHeight(900, 80)).toBe(320);
    const source = readFileSync(new URL("./ProviderCommandMenu.tsx", import.meta.url), "utf8");
    expect(source).toContain('scrollIntoView({ block: "nearest" })');
    expect(source).toContain("props.onSubmit !== undefined && command.argumentHint === undefined");
  });

  it("shows a concise preview while preserving the full provider description", () => {
    const description = "Review every changed file and provide a full adversarial report. "
      + "This second sentence contains implementation detail that should not dominate the menu. "
      + "Additional provider prose remains available to assistive technology.";
    const html = renderToStaticMarkup(createElement(ProviderCommandMenu, {
      id: "commands", providerLabel: "Codex", activeIndex: 0,
      commands: [{ name: "review", description }],
      onChoose: () => undefined, onActiveIndexChange: () => undefined,
    }));
    expect(providerCommandDescriptionPreview(description))
      .toBe("Review every changed file and provide a full adversarial report.");
    expect(html).toContain("Review every changed file and provide a full adversarial report.");
    expect(html).toContain(`title="${description}"`);
    expect(html).toContain(`<span class="provider-command-sr-only">${description}</span>`);
  });

  it("suppresses the repeated fallback description and bounds a long first sentence", () => {
    expect(providerCommandDescriptionPreview("Provider command")).toBe("");
    const preview = providerCommandDescriptionPreview("x".repeat(480));
    expect(preview).toHaveLength(120);
    expect(preview.endsWith("…")).toBe(true);
  });
});
