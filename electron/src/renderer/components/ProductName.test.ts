import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { isDevBuild, productLabel } from "../product";
import { ProductName } from "./ProductName";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("product name", () => {
  it("is Interface on the release channel", () => {
    expect(isDevBuild()).toBe(false);
    expect(productLabel()).toBe("Interface");
    expect(renderToStaticMarkup(createElement(ProductName))).toBe("Interface");
  });

  it("appends a canary Dev chip on the dev channel", () => {
    vi.stubGlobal("window", { ifaceBuild: { channel: "dev" } });
    expect(productLabel()).toBe("Interface Dev");
    const html = renderToStaticMarkup(createElement(ProductName, { suffix: " style" }));
    expect(html).toContain("Interface");
    expect(html).toContain('class="product-dev-badge"');
    expect(html).toContain("Dev");
    expect(html).toContain(" style");
  });

  it("marks every in-app product wordmark with the shared component", () => {
    const sources = [
      new URL("../App.tsx", import.meta.url),
      new URL("./Composer.tsx", import.meta.url),
      new URL("./ConversationTranscript.tsx", import.meta.url),
      new URL("../main.tsx", import.meta.url),
    ].map(url => readFileSync(url, "utf8"));
    for (const source of sources) {
      expect(source).toContain("<ProductName");
    }
  });

  it("builds a distinct Dev dock icon and installs it on Interface Dev.app", () => {
    const icon = readFileSync(new URL("../../../build/icon-dev.svg", import.meta.url), "utf8");
    const production = readFileSync(new URL("../../../build/icon.svg", import.meta.url), "utf8");
    const makeIcon = readFileSync(new URL("../../../scripts/make-icon.mjs", import.meta.url), "utf8");
    const dev = readFileSync(new URL("../../../scripts/dev.mjs", import.meta.url), "utf8");
    expect(icon).toContain("#f0c33c");
    expect(icon).toContain("#1c1608");
    expect(production).toContain("#128a55");
    expect(production).not.toContain("#f0c33c");
    expect(makeIcon).toContain("icon-dev.svg");
    expect(dev).toContain("icon-dev.icns");
  });
});
