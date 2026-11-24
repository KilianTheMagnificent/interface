import { createHash } from "node:crypto";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  PROVIDER_LOGO_CSS, ProviderLogo, providerInitials,
  providerKindFromSession, providerLogoKind,
} from "./ProviderLogo";

/**
 * The points an absolute M/L/H/V/C/Z outline passes through, densely enough
 * that clipping them to a viewBox measures the drawn mark rather than the
 * asset's bounding box.
 */
function outlinePoints(d: string, steps = 256): Array<[number, number]> {
  const tokens = d.match(/[A-Za-z]|-?\d*\.?\d+/gu) ?? [];
  const points: Array<[number, number]> = [];
  let index = 0;
  let x = 0;
  let y = 0;
  let startX = 0;
  let startY = 0;
  const next = (): number => Number(tokens[index++]);
  while (index < tokens.length) {
    const command = tokens[index++];
    if (command === "M" || command === "L") {
      x = next(); y = next();
      if (command === "M") { startX = x; startY = y; }
    } else if (command === "H") {
      x = next();
    } else if (command === "V") {
      y = next();
    } else if (command === "C") {
      const [x1, y1, x2, y2, x3, y3] = [next(), next(), next(), next(), next(), next()];
      for (let step = 1; step <= steps; step += 1) {
        const t = step / steps;
        const u = 1 - t;
        points.push([
          u ** 3 * x + 3 * u ** 2 * t * x1 + 3 * u * t ** 2 * x2 + t ** 3 * x3,
          u ** 3 * y + 3 * u ** 2 * t * y1 + 3 * u * t ** 2 * y2 + t ** 3 * y3,
        ]);
      }
      x = x3; y = y3;
      continue;
    } else if (command === "Z") {
      x = startX; y = startY;
    } else {
      throw new Error(`Unsupported path command ${command}`);
    }
    points.push([x, y]);
  }
  return points;
}

describe("ProviderLogo", () => {
  it("maps only verified built-ins to brand marks", () => {
    expect(providerLogoKind("claude")).toBe("claude");
    expect(providerLogoKind("codex")).toBe("codex");
    expect(providerLogoKind("acp:grok")).toBe("grok");
    expect(providerLogoKind("acp:qwen")).toBe("qwen");
    expect(providerLogoKind("acp:gemini")).toBe("gemini");
    expect(providerLogoKind("gemini")).toBe("gemini");
    expect(providerLogoKind("acp:local-agent")).toBe("custom");
    expect(providerKindFromSession("claude")).toBe("claude");
    expect(providerKindFromSession(undefined, "codex#abc")).toBe("codex");
    expect(providerKindFromSession("", "acp:grok#1")).toBe("grok");
    expect(providerKindFromSession()).toBe("custom");
  });

  it("draws the Gemini spark for the acp:gemini profile", () => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "acp:gemini", label: "Gemini", size: 18,
    }));
    expect(markup).toContain('data-provider-logo="gemini"');
    expect(markup).toContain('class="provider-logo provider-logo-gemini"');
    expect(markup).toContain('fill="currentColor"');
    expect(markup).toContain('aria-hidden="true"');
    expect(PROVIDER_LOGO_CSS).toContain(".provider-logo-gemini { color:#4285f4; }");
    expect(PROVIDER_LOGO_CSS).toContain(
      '[data-provider="gemini"] { --provider-accent:#4285f4; --working-spark:var(--provider-accent); }',
    );
  });

  it("draws the official Antigravity arch while preserving the stable Gemini engine", () => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "acp:gemini", label: "Antigravity", size: 18,
    }));
    expect(providerLogoKind("acp:gemini")).toBe("gemini");
    expect(markup).toContain('data-provider-logo="antigravity"');
    expect(markup).toContain('class="provider-logo provider-logo-antigravity"');
    // One colour like every other mark: the official arch, not a hand-cut
    // five-colour A.
    expect(markup.match(/<path /gu)).toHaveLength(1);
    expect(markup).toContain('fill="currentColor" d="M21.751 22.607');
    expect(PROVIDER_LOGO_CSS)
      .toContain(".provider-logo-antigravity { color:#4285f4; }");
  });

  it("draws Qwen's official knot, a solid π for OMP and OpenCode at the shared weight", () => {
    const qwen = renderToStaticMarkup(createElement(ProviderLogo, { engine: "acp:qwen", label: "Qwen", size: 16 }));
    expect(qwen).toContain('d="M12.604 1.34c.393.69');
    expect(qwen).toContain('fill-rule="evenodd"');
    const omp = renderToStaticMarkup(createElement(ProviderLogo, { engine: "omp", label: "OMP", size: 16 }));
    expect(omp).toContain('stroke-width="3"');
    const opencode = renderToStaticMarkup(createElement(ProviderLogo, { engine: "opencode", label: "OpenCode", size: 16 }));
    // The block is padded to 85% of the slot so it stops reading as a black tile.
    expect(opencode).toContain('viewBox="-21.18 -26.47 282.35 352.94"');
  });

  it("draws Muse Code with the official Muse squiggle in Muse blue", () => {
    // SHA-256 of the unmodified path from Muse's official favicon (2026-10-18):
    // https://muse.ai/images/favicon/app-squiggle.svg
    for (const engine of ["muse", "acp:muse"]) {
      expect(providerLogoKind(engine)).toBe("muse");
      const muse = renderToStaticMarkup(createElement(ProviderLogo, {
        engine, label: "Muse Code", size: 16,
      }));
      expect(muse).toContain('data-provider-logo="muse"');
      expect(muse).toContain('viewBox="0 0 100 100"');
      expect(muse).toContain('width="16" height="16"');
      expect(muse).toContain('fill="currentColor"');
      expect(muse).toContain('aria-hidden="true"');
      expect(muse).not.toContain('stroke-width=');
      const paths = [...muse.matchAll(/<path [^>]*d="([^"]+)"/gu)].map(match => match[1]);
      expect(paths).toHaveLength(1);
      expect(createHash("sha256").update(paths[0]).digest("hex"))
        .toBe("2931eaf95e2b87ce146b337282d675721602743f4eb840850692470dbd68f74c");
    }
    // The mark and the provider accent (pips, working spark, model chip) share
    // two stops of Muse's official gradient, picked by the theme's scheme.
    expect(PROVIDER_LOGO_CSS).toContain(".provider-logo-muse { color:light-dark(#0064e0,#0082fb); }");
    expect(PROVIDER_LOGO_CSS).toContain(
      '[data-provider="muse"] { --provider-accent:light-dark(#0064e0,#0082fb); --working-spark:var(--provider-accent); }',
    );
  });

  it("draws Kimi Code as Lobe Icons' Kimi K in ink with the Kimi-blue dot", () => {
    for (const engine of ["kimi", "acp:kimi"]) {
      expect(providerLogoKind(engine)).toBe("kimi");
      const kimi = renderToStaticMarkup(createElement(ProviderLogo, {
        engine, label: "Kimi Code", size: 16,
      }));
      expect(kimi).toContain('data-provider-logo="kimi"');
      expect(kimi).toContain('viewBox="0 0 24 24"');
      expect(kimi).toContain('width="16" height="16"');
      expect(kimi).toContain('aria-hidden="true"');
      const fills = [...kimi.matchAll(/<path fill="([^"]+)"/gu)].map(match => match[1]);
      expect(fills).toEqual(["#1783ff", "currentColor"]);
    }
    expect(providerKindFromSession(undefined, "acp:kimi#session_1")).toBe("kimi");
    expect(PROVIDER_LOGO_CSS).toContain(".provider-logo.provider-logo-kimi { color:light-dark(#000,#fff); }");
    expect(PROVIDER_LOGO_CSS).toContain(
      '[data-provider="kimi"] { --provider-accent:#1783ff; --working-spark:var(--provider-accent); }',
    );
  });

  it("draws the SpaceXAI symbol for Grok provider sessions", () => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "acp:grok", label: "Grok", size: 18,
    }));
    expect(markup).toContain('data-provider-logo="grok"');
    expect(markup).toContain('viewBox="0 0 382 382"');
    expect(markup).toContain('preserveAspectRatio="xMidYMid"');
    expect(markup).toContain('overflow="hidden"');
    const paths = [...markup.matchAll(/<path [^>]*d="([^"]+)"/gu)]
      .map(match => match[1]);
    expect(paths).toHaveLength(4);
    expect(createHash("sha256").update(paths.join("\n")).digest("hex"))
      .toBe("cc971b30cc6fafb775d9a9830718cfa39d20f562afae87bf3cbda9f06b0573a3");
    expect(markup.match(/fill="currentColor"/gu)).toHaveLength(4);
    expect(markup).not.toContain("<rect");
    expect(markup).toContain("M900.551 99.7843C804.619 107.606");
    expect(markup).not.toContain("466.04 516.93");
  });

  it("fills the squared SpaceXAI slot edge to edge at the tallest ink it allows", () => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "acp:grok", label: "Grok", size: 12,
    }));
    const viewBox = markup.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/u);
    expect(viewBox).not.toBeNull();
    const side = Number(viewBox?.[1]);
    expect(side).toBe(Number(viewBox?.[2]));

    // Measure what is DRAWN, not the asset's bounding box. The asset reaches
    // y=99.49 at x≈900, which the square crop clips away, so the uncropped
    // 317.7-unit height certifies a parity the frame never shows. Sample the
    // outlines, apply the crop's translate, drop every point outside the
    // viewBox, and take the bbox of what is left.
    const translate = markup.match(/translate\((-?[\d.]+) (-?[\d.]+)\)/u);
    expect(translate).not.toBeNull();
    const dx = Number(translate?.[1]);
    const dy = Number(translate?.[2]);
    const drawn = [...markup.matchAll(/<path [^>]*d="([^"]+)"/gu)]
      .flatMap(match => outlinePoints(match[1]))
      .map(([x, y]) => [x + dx, y + dy] as const)
      .filter(([x, y]) => x >= 0 && x <= side && y >= 0 && y <= side);
    expect(drawn.length).toBeGreaterThan(0);
    const span = (values: number[]): number => Math.max(...values) - Math.min(...values);
    const inkHeight = 12 * span(drawn.map(([, y]) => y)) / side;
    const inkWidth = 12 * span(drawn.map(([x]) => x)) / side;

    // Full height parity with Claude, Codex, Gemini and Qwen (9.75-12px of ink
    // in this same 12px box) is not reachable: the asset is 831 x 318, and a
    // 2.6:1 mark cannot fill a square slot's height without losing most of its
    // width. What the crop buys, sampled the same way: 5.48px of visible ink
    // height before, 7.22px now, at the full width of the slot. A taller mark
    // needs a wider slot, not a different crop.
    expect(inkHeight).toBeGreaterThanOrEqual(7);
    expect(inkWidth).toBeGreaterThanOrEqual(11.5);
    // The crop is a translate, so the official path data is untouched.
    expect(markup).toContain('<g transform="translate(-69.0684 -107)">');
  });

  it("uses configured-provider initials without guessing another brand", () => {
    expect(providerInitials("Local Agent")).toBe("LA");
    expect(providerInitials("gemini")).toBe("GE");
    expect(providerInitials("custom_provider")).toBe("CP");
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "acp:local-agent", label: "Local Agent",
    }));
    expect(markup).toContain('data-provider-logo="custom"');
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).toContain(">LA</span>");
  });

  it("drops to one initial below 16px so 11px type cannot overhang the tile", () => {
    const props = { engine: "acp:local-agent", label: "Local Agent" };
    expect(renderToStaticMarkup(createElement(ProviderLogo, { ...props, size: 14 })))
      .toContain(">L</span>");
    expect(renderToStaticMarkup(createElement(ProviderLogo, { ...props, size: 16 })))
      .toContain(">LA</span>");
    expect(renderToStaticMarkup(createElement(ProviderLogo, { ...props, size: 20 })))
      .toContain(">LA</span>");
  });

  it.each([
    ["claude", "Claude", "claude"],
    ["codex", "Codex", "codex"],
    ["acp:grok", "Grok", "grok"],
    ["acp:qwen", "Qwen", "qwen"],
  ])("renders the %s mark as decorative inline SVG", (engine, label, mark) => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, { engine, label }));
    expect(markup).toContain(`<svg class="provider-logo provider-logo-${mark}"`);
    expect(markup).toContain(`data-provider-logo="${mark}"`);
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).toContain('focusable="false"');
    expect(markup).not.toContain("<img");
    expect(markup).not.toContain("width=");
  });

  it("pins intrinsic size so a wide viewBox cannot inflate a flex parent", () => {
    const markup = renderToStaticMarkup(createElement(ProviderLogo, {
      engine: "codex", label: "Codex", size: 12,
    }));
    expect(markup).toContain('width="12"');
    expect(markup).toContain('height="12"');
    expect(markup).toContain("width:12px");
    expect(markup).toContain("min-width:0");
  });

  it("owns the shared visual contract", () => {
    expect(PROVIDER_LOGO_CSS).toContain(".provider-logo-fallback");
    expect(PROVIDER_LOGO_CSS).toContain(".provider-logo-codex { color:#12805a; }");
    expect(PROVIDER_LOGO_CSS)
      .toContain(".provider-logo.provider-logo-grok { color:light-dark(#000,#fff); }");
    for (const rule of [
      '[data-provider="claude"] { --provider-accent:#b84f31; --working-spark:var(--provider-accent); }',
      '[data-provider="codex"] { --provider-accent:#12805a; --working-spark:var(--provider-accent); }',
      '[data-provider="grok"] { --provider-accent:var(--fg-strong); --working-spark:var(--provider-accent); }',
      '[data-provider="qwen"] { --provider-accent:#615ced; --working-spark:var(--provider-accent); }',
    ]) expect(PROVIDER_LOGO_CSS).toContain(rule);
  });
});
