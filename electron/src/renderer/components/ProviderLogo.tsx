/**
 * Provider-mark geometry follows T3 Code's MIT-licensed inline icon approach.
 * The Qwen, Antigravity and Kimi outlines are the marks from Lobe Icons
 * (https://github.com/lobehub/lobe-icons, MIT License, Copyright (c) 2023
 * LobeHub). Brand marks remain the property of their respective owners.
 */
export type ProviderLogoKind = "claude" | "codex" | "grok" | "qwen" | "gemini"
  | "antigravity" | "omp" | "opencode" | "muse" | "kimi" | "custom";

/**
 * Provider marks are used outside AppSelect (agent tiles, Settings and Usage),
 * so their complete visual contract lives with the component. AppSelect
 * includes this string once at the renderer root to avoid repeating a style
 * element for every logo instance.
 */
export const PROVIDER_LOGO_CSS = `
.provider-logo { display:inline-flex; flex:none; width:100%; height:100%; color:var(--fg); }
.provider-logo-claude { color:#b84f31; }
.provider-logo-codex { color:#12805a; }
.provider-logo.provider-logo-grok { color:light-dark(#000,#fff); }
.provider-logo.provider-logo-opencode { color:light-dark(#211e1e,#f1ecec); }
.provider-logo-qwen { color:#615ced; }
.provider-logo-gemini { color:#4285f4; }
.provider-logo-antigravity { color:#4285f4; }
/* Muse blue: the official gradient's deep stop on light themes, its bright stop on dark. */
.provider-logo-muse { color:light-dark(#0064e0,#0082fb); }
/* Kimi's K is ink like Grok's; its brand blue lives in the dot and the accent. */
.provider-logo.provider-logo-kimi { color:light-dark(#000,#fff); }
/* Provider colour is identity, not state. Containers may use the same hue for
   a restrained tint, while the adjacent label continues to carry the meaning.
   Working sparks inherit it because the visible word already names the state. */
[data-provider="claude"] { --provider-accent:#b84f31; --working-spark:var(--provider-accent); }
[data-provider="codex"] { --provider-accent:#12805a; --working-spark:var(--provider-accent); }
[data-provider="grok"] { --provider-accent:var(--fg-strong); --working-spark:var(--provider-accent); }
[data-provider="qwen"] { --provider-accent:#615ced; --working-spark:var(--provider-accent); }
[data-provider="gemini"] { --provider-accent:#4285f4; --working-spark:var(--provider-accent); }
[data-provider="opencode"] { --provider-accent:var(--fg-strong); --working-spark:var(--provider-accent); }
[data-provider="omp"] { --provider-accent:var(--fg-strong); --working-spark:var(--provider-accent); }
[data-provider="muse"] { --provider-accent:light-dark(#0064e0,#0082fb); --working-spark:var(--provider-accent); }
[data-provider="kimi"] { --provider-accent:#1783ff; --working-spark:var(--provider-accent); }
[data-provider="custom"] { --provider-accent:var(--accent-text); --working-spark:var(--status-working); }
.provider-logo-fallback { align-items:center; justify-content:center; box-sizing:border-box;
  color:var(--fg-muted); background:var(--surface-raised);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font:var(--wght-strong) var(--text-meta)/1 var(--font-ui); }
`;

export function providerLogoKind(engine: string): ProviderLogoKind {
  if (engine === "opencode" || engine === "acp:opencode") return "opencode";
  if (engine === "omp" || engine === "acp:omp") return "omp";
  if (engine === "muse" || engine === "acp:muse") return "muse";
  if (engine === "kimi" || engine === "acp:kimi") return "kimi";
  if (engine === "claude") return "claude";
  if (engine === "codex") return "codex";
  if (engine === "acp:grok" || engine === "grok") return "grok";
  if (engine === "acp:qwen" || engine === "qwen") return "qwen";
  if (engine === "acp:gemini" || engine === "gemini") return "gemini";
  return "custom";
}

/** Engine id first; session keys are `engine#id` when the config has not arrived. */
export function providerKindFromSession(engine?: string, sessionKey?: string): ProviderLogoKind {
  if (engine !== undefined && engine !== "") return providerLogoKind(engine);
  const prefix = sessionKey?.split("#", 1)[0] ?? "";
  return providerLogoKind(prefix);
}

/** T3-style deterministic fallback for provider profiles without a known mark. */
export function providerInitials(label: string): string {
  const words = label.replace(/[_-]+/g, " ").trim().split(/\s+/u).filter(Boolean);
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0].slice(0, 2).toLocaleUpperCase();
  return words.slice(0, 2).map(word => word[0]?.toLocaleUpperCase() ?? "").join("");
}

export function ProviderLogo({ engine, label, className = "", size }: {
  engine: string;
  label: string;
  className?: string;
  /** Pixel box. Sets the SVG's intrinsic size so a 256-wide viewBox cannot
   *  expand a flex parent. */
  size?: number;
}) {
  const kind = (engine === "acp:gemini" || engine === "gemini")
      && label.trim().toLocaleLowerCase() === "antigravity"
    ? "antigravity" : providerLogoKind(engine);
  const classes = ["provider-logo", `provider-logo-${kind}`, className]
    .filter(Boolean).join(" ");
  const sized = size === undefined ? undefined : {
    width: size, height: size, minWidth: 0, minHeight: 0,
  };

  if (kind === "claude") {
    return <svg className={classes} data-provider-logo="claude" viewBox="0 0 24 24"
      width={size} height={size} style={sized}
      aria-hidden="true" focusable="false">
      <path fill="currentColor" d="m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" />
    </svg>;
  }

  if (kind === "codex") {
    return <svg className={classes} data-provider-logo="codex" viewBox="0 0 256 260"
      width={size} height={size} style={sized}
      preserveAspectRatio="xMidYMid" aria-hidden="true" focusable="false">
      <path fill="currentColor" d="M239.184 106.203a64.716 64.716 0 0 0-5.576-53.103C219.452 28.459 191 15.784 163.213 21.74A65.586 65.586 0 0 0 52.096 45.22a64.716 64.716 0 0 0-43.23 31.36c-14.31 24.602-11.061 55.634 8.033 76.74a64.665 64.665 0 0 0 5.525 53.102c14.174 24.65 42.644 37.324 70.446 31.36a64.72 64.72 0 0 0 48.754 21.744c28.481.025 53.714-18.361 62.414-45.481a64.767 64.767 0 0 0 43.229-31.36c14.137-24.558 10.875-55.423-8.083-76.483Zm-97.56 136.338a48.397 48.397 0 0 1-31.105-11.255l1.535-.87 51.67-29.825a8.595 8.595 0 0 0 4.247-7.367v-72.85l21.845 12.636c.218.111.37.32.409.563v60.367c-.056 26.818-21.783 48.545-48.601 48.601Zm-104.466-44.61a48.345 48.345 0 0 1-5.781-32.589l1.534.921 51.722 29.826a8.339 8.339 0 0 0 8.441 0l63.181-36.425v25.221a.87.87 0 0 1-.358.665l-52.335 30.184c-23.257 13.398-52.97 5.431-66.404-17.803ZM23.549 85.38a48.499 48.499 0 0 1 25.58-21.333v61.39a8.288 8.288 0 0 0 4.195 7.316l62.874 36.272-21.845 12.636a.819.819 0 0 1-.767 0L41.353 151.53c-23.211-13.454-31.171-43.144-17.804-66.405v.256Zm179.466 41.695-63.08-36.63L161.73 77.86a.819.819 0 0 1 .768 0l52.233 30.184a48.6 48.6 0 0 1-7.316 87.635v-61.391a8.544 8.544 0 0 0-4.4-7.213Zm21.742-32.69-1.535-.922-51.619-30.081a8.39 8.39 0 0 0-8.492 0L99.98 99.808V74.587a.716.716 0 0 1 .307-.665l52.233-30.133a48.652 48.652 0 0 1 72.236 50.391v.205ZM88.061 139.097l-21.845-12.585a.87.87 0 0 1-.41-.614V65.685a48.652 48.652 0 0 1 79.757-37.346l-1.535.87-51.67 29.825a8.595 8.595 0 0 0-4.246 7.367l-.051 72.697Zm11.868-25.58 28.138-16.217 28.188 16.218v32.434l-28.086 16.218-28.188-16.218-.052-32.434Z" />
    </svg>;
  }

  if (kind === "grok") {
    // Official squared SpaceXAI symbol geometry, drawn at the scale its
    // neighbours are drawn at. The asset is 831 wide and 318 tall, so a square
    // crop is the standing decision here; the box was 600 across, which left
    // the ink filling barely half the height that the Claude, Codex, Gemini and
    // Qwen marks fill, and the mark read as the small one in every row that
    // showed two. 382 is the glyph's own height plus the ~17% margin those four
    // carry, so all five now draw at one ink scale. The translate anchors the
    // crop at the mark's left tip and centres the ink that stays in frame;
    // currentColor still supplies the asset's black/white theme treatment
    // without tinting it.
    return <svg className={classes} data-provider-logo="grok" viewBox="0 0 382 382"
      width={size} height={size} style={sized}
      preserveAspectRatio="xMidYMid" overflow="hidden"
      aria-hidden="true" focusable="false">
      <g transform="translate(-69.0684 -107)">
        <path fill="currentColor" d="M900.551 99.7843C804.619 107.606 427.549 154.353 166.092 417.192H69.0684L79.9091 406.388C134.623 353.529 376.727 129.506 900.551 99.4932V99.7843Z" />
        <path fill="currentColor" d="M572.178 417.192H496.133L346.301 308.126C360.108 299.443 374.004 291.173 387.943 283.297L572.178 417.192Z" />
        <path fill="currentColor" d="M450.645 417.193H374.634L351.576 400.422H222.839C231.902 392.374 241.077 384.561 250.349 376.977H319.308L283.623 351.019C296.378 341.528 309.274 332.437 322.274 323.728L450.645 417.193Z" />
        <path fill="currentColor" d="M173.984 216.084L232.598 258.681C218.01 267.271 204.357 275.774 191.613 284.091L98.0385 216.047L173.984 216.084Z" />
      </g>
    </svg>;
  }

  if (kind === "qwen") {
    // Qwen's official hexagonal knot (Lobe Icons' monochrome outline).
    return <svg className={classes} data-provider-logo="qwen" viewBox="0 0 24 24"
      width={size} height={size} style={sized}
      aria-hidden="true" focusable="false">
      <path fill="currentColor" fillRule="evenodd" d="M12.604 1.34c.393.69.784 1.382 1.174 2.075a.18.18 0 00.157.091h5.552c.174 0 .322.11.446.327l1.454 2.57c.19.337.24.478.024.837-.26.43-.513.864-.76 1.3l-.367.658c-.106.196-.223.28-.04.512l2.652 4.637c.172.301.111.494-.043.77-.437.785-.882 1.564-1.335 2.34-.159.272-.352.375-.68.37-.777-.016-1.552-.01-2.327.016a.099.099 0 00-.081.05 575.097 575.097 0 01-2.705 4.74c-.169.293-.38.363-.725.364-.997.003-2.002.004-3.017.002a.537.537 0 01-.465-.271l-1.335-2.323a.09.09 0 00-.083-.049H4.982c-.285.03-.553-.001-.805-.092l-1.603-2.77a.543.543 0 01-.002-.54l1.207-2.12a.198.198 0 000-.197 550.951 550.951 0 01-1.875-3.272l-.79-1.395c-.16-.31-.173-.496.095-.965.465-.813.927-1.625 1.387-2.436.132-.234.304-.334.584-.335a338.3 338.3 0 012.589-.001.124.124 0 00.107-.063l2.806-4.895a.488.488 0 01.422-.246c.524-.001 1.053 0 1.583-.006L11.704 1c.341-.003.724.032.9.34zm-3.432.403a.06.06 0 00-.052.03L6.254 6.788a.157.157 0 01-.135.078H3.253c-.056 0-.07.025-.041.074l5.81 10.156c.025.042.013.062-.034.063l-2.795.015a.218.218 0 00-.2.116l-1.32 2.31c-.044.078-.021.118.068.118l5.716.008c.046 0 .08.02.104.061l1.403 2.454c.046.081.092.082.139 0l5.006-8.76.783-1.382a.055.055 0 01.096 0l1.424 2.53a.122.122 0 00.107.062l2.763-.02a.04.04 0 00.035-.02.041.041 0 000-.04l-2.9-5.086a.108.108 0 010-.113l.293-.507 1.12-1.977c.024-.041.012-.062-.035-.062H9.2c-.059 0-.073-.026-.043-.077l1.434-2.505a.107.107 0 000-.114L9.225 1.774a.06.06 0 00-.053-.031zm6.29 8.02c.046 0 .058.02.034.06l-.832 1.465-2.613 4.585a.056.056 0 01-.05.029.058.058 0 01-.05-.029L8.498 9.841c-.02-.034-.01-.052.028-.054l.216-.012 6.722-.012z" />
    </svg>;
  }

  if (kind === "antigravity") {
    // Antigravity's official arch (Lobe Icons' monochrome outline) in one
    // colour, like every other mark here. Keep the stable acp:gemini engine id
    // while presenting the product the CLI opens.
    return <svg className={classes} data-provider-logo="antigravity" viewBox="0 0 24 24"
      width={size} height={size} style={sized}
      aria-hidden="true" focusable="false">
      <path fill="currentColor" d="M21.751 22.607c1.34 1.005 3.35.335 1.508-1.508C17.73 15.74 18.904 1 12.037 1 5.17 1 6.342 15.74.815 21.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z" />
    </svg>;
  }

  if (kind === "opencode") {
    // Official mark and theme palettes: https://opencode.ai/brand. The viewBox
    // pads the 240 x 300 block to 85% so its solid mass weighs what the
    // outlined marks beside it weigh, rather than reading as a black tile.
    return <svg className={classes} data-provider-logo="opencode" viewBox="-21.18 -26.47 282.35 352.94"
      width={size} height={size} style={sized} focusable="false" aria-hidden="true"
      fill="currentColor">
      <path d="M180 240H60V120H180V240Z" fill="light-dark(#cfcecd,#4b4646)" />
      <path d="M180 60H60V240H180V60ZM240 300H0V0H240V300Z" />
    </svg>;
  }
  if (kind === "omp") {
    // A bold π drawn to the same ink box as the brand marks, so it reads as a
    // mark rather than a typed character.
    return <svg className={classes} data-provider-logo="omp" viewBox="0 0 24 24"
      width={size} height={size} style={sized} focusable="false" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="3"
      strokeLinecap="round" strokeLinejoin="round">
      <path d="M3.5 5.5h17M9 5.5v6c0 3.6-1.3 6.2-2.8 7.8M15.5 5.5v10.2c0 2.8 2.4 3.4 5 2.3" />
    </svg>;
  }
  if (kind === "muse") {
    // Muse's official squiggle; preserve the original outline and square canvas.
    // Source: https://muse.ai/images/favicon/app-squiggle.svg (2026-10-18).
    // As with the other provider marks, one brand colour (Muse blue) replaces the gradient.
    return <svg className={classes} data-provider-logo="muse" viewBox="0 0 100 100"
      width={size} height={size} style={sized} focusable="false" aria-hidden="true">
      <path fill="currentColor" d="M24.6257 16.1214C28.2365 16.1214 30.8187 18.9408 30.8187 23.3643C30.8185 27.7868 30.1055 31.7765 28.9225 40.2466C28.0743 46.1813 26.62 55.2418 26.0132 59.0088C27.1782 56.7815 29.9074 51.5757 30.3914 50.5982C36.1081 39.061 42.0657 28.7544 43.7826 26.0254C46.0594 22.382 48.062 20.9229 51.1597 20.9229C52.7685 20.923 54.6826 21.7357 55.6315 22.819C56.9178 24.2895 57.1859 26.6602 57.1411 29.244C57.0738 33.2073 56.5599 39.5264 55.8553 46.5698C55.5416 49.7057 54.9365 56.1066 54.5288 60.4004C58.1883 53.8532 64.9609 41.8109 67.6921 37.6058C70.0727 33.9402 72.466 31.5797 75.6836 31.5796C78.4595 31.5796 81.8809 33.5416 81.6772 38.7573C81.3774 46.4556 80.2673 63.2232 79.7607 70.7479C83.1662 65.7913 87.6377 58.4543 89.3148 55.6966C91.0104 52.908 92.75 50.6542 95.3898 50.5778C98.0987 50.4995 99.913 52.341 99.9959 55.1839C100.078 58.0277 98.3835 61.2256 96.1344 64.856C93.6937 68.7957 90.9621 73.0123 87.6383 76.9491C84.3148 80.8856 81.2604 83.8782 76.5991 83.8786C72.4243 83.8786 69.4869 81.1043 69.283 76.0254C69.1768 73.364 69.6804 67.6405 70.0602 63.1673C70.5 57.9875 71.0315 52.9861 71.403 49.6786C67.7337 56.3979 60.0101 70.4676 56.5348 75.9603C53.1343 81.3339 51.4774 83.2269 47.8882 83.2275C44.2977 83.2275 42.7226 80.6416 42.6717 76.7497C42.6142 72.5285 45.7852 47.1821 46.8587 38.737C45.1183 42.4616 40.839 51.6056 38.1551 57.0435C35.5587 62.3055 31.204 71.3978 28.0436 76.6357C25.7668 80.4094 23.2092 83.2273 19.4173 83.2275C15.6252 83.2275 13.4847 80.6784 13.4847 76.5788C13.4847 72.4794 14.6716 65.6492 15.9465 57.1777C16.5839 53.0781 17.3108 48.6133 18.1315 43.8761C18.7324 40.448 19.7764 34.4169 20.4183 30.7129C17.502 35.2539 12.2536 43.4278 10.6567 45.8903C8.88113 48.6291 7.28747 50.626 4.64681 50.6266C1.7329 50.6266 0.000532901 48.3522 0 45.7113C0 43.0703 1.34323 41.0344 3.52376 37.4512C6.32403 32.8465 11.3578 26.57 14.1154 23.2747C17.3656 19.3912 20.6139 16.1219 24.6257 16.1214Z" />
    </svg>;
  }
  if (kind === "kimi") {
    // Lobe Icons' Kimi mark (kimi-color.svg, @lobehub/icons-static-svg 1.98.0):
    // the K in ink and the dot in Kimi blue, as Moonshot draws it.
    return <svg className={classes} data-provider-logo="kimi" viewBox="0 0 24 24"
      width={size} height={size} style={sized} focusable="false" aria-hidden="true">
      <path fill="#1783ff" d="M21.846 0a1.923 1.923 0 110 3.846H20.15a.226.226 0 01-.227-.226V1.923C19.923.861 20.784 0 21.846 0z" />
      <path fill="currentColor" d="M11.065 11.199l7.257-7.2c.137-.136.06-.41-.116-.41H14.3a.164.164 0 00-.117.051l-7.82 7.756c-.122.12-.302.013-.302-.179V3.82c0-.127-.083-.23-.185-.23H3.186c-.103 0-.186.103-.186.23V19.77c0 .128.083.23.186.23h2.69c.103 0 .186-.102.186-.23v-3.25c0-.069.025-.135.069-.178l2.424-2.406a.158.158 0 01.205-.023l6.484 4.772a7.677 7.677 0 003.453 1.283c.108.012.2-.095.2-.23v-3.06c0-.117-.07-.212-.164-.227a5.028 5.028 0 01-2.027-.807l-5.613-4.064c-.117-.078-.132-.279-.028-.381z" />
    </svg>;
  }
  if (kind === "gemini") {
    // Gemini's four-point spark: four quarter-circle sweeps between the points,
    // drawn here rather than copied so no upstream asset ships with the app.
    return <svg className={classes} data-provider-logo="gemini" viewBox="0 0 24 24"
      width={size} height={size} style={sized}
      aria-hidden="true" focusable="false">
      <path fill="currentColor" d="M12 1.5c0 5.799 4.701 10.5 10.5 10.5C16.701 12 12 16.701 12 22.5 12 16.701 7.299 12 1.5 12 7.299 12 12 7.299 12 1.5Z" />
    </svg>;
  }

  /* Two initials need about 14px of advance at --text-meta, which overhangs a
     tile smaller than that plus its borders. The mark is decorative and the
     adjacent label carries the name, so below 16px the first letter stands in
     rather than dropping the type under the 11px floor. */
  const initials = providerInitials(label);
  return <span className={`${classes} provider-logo-fallback`} data-provider-logo="custom"
    style={sized} aria-hidden="true">{
      size !== undefined && size < 16 ? initials.slice(0, 1) : initials
    }</span>;
}
