# Theme files

Interface themes are plain CSS files, the way Obsidian's are. Put a file in

    ~/.config/interface/themes/<Name>.css

and it appears in **Settings › Appearance** under the **Your themes** tab,
beside the built-in **Light** and **Dark** tabs (a theme whose backdrop moves
is listed first instead, under **Live backgrounds**). Pick it there, or pick it as the light or dark theme that
**Follow macOS** switches to. Edit the file in any editor: when you save,
Interface repaints within about a second. Nothing needs restarting.

The folder is created the first time Interface starts. It may be a link (for
example into a dotfiles repository), and so may the files in it. Interface
only reads theme files; it never writes or deletes them.

## The smallest theme

```css
:root {
  --theme-kind: dark;
  --theme-background: #0f1310;
  --theme-text: #dde5d9;
  --theme-accent: #9fd4a8;
}
```

`--theme-kind` says whether the theme is `light` or `dark`. It decides the
native window controls, which built-in colours fill in what the file leaves
out, and the starting terminal colours. Without it, a hex `--editor-bg` or
`--theme-background` decides; with neither, the theme is dark.

## Key colours

Eight key colours colour the whole window. Every other colour token is worked
out from them unless the file sets it too. Unset keys default to Porcelain
Night (dark) or Porcelain (light).

| Key | Paints | Defaults to |
| --- | --- | --- |
| `--theme-background` | The canvas behind panes, the editor | Porcelain (Night)'s canvas |
| `--theme-panel` | Sidebar, Settings, pane frames | `--theme-background` |
| `--theme-raised` | Cards, menus, dialogs, inputs | Porcelain (Night)'s surface |
| `--theme-text` | Every label and row (muted tiers are mixed from it) | Porcelain (Night)'s text |
| `--theme-accent` | Primary buttons, links, switches | Porcelain (Night)'s accent |
| `--theme-selection` | Focus rings, selected rows, badges | Porcelain (Night)'s blue |
| `--theme-screen` | Behind terminal text | near-black (dark), near-white (light) |
| `--theme-screen-text` | Default terminal text | `--theme-text` |

## Meaning colours

Four more colours carry meaning. Set them and the status lights, diffs,
access modes and the terminal's red, green, yellow and magenta all follow, so
a pane and a thread show a failure in the same red:

| Key | Paints |
| --- | --- |
| `--theme-danger` | Failed status, removed lines, danger mode, terminal red |
| `--theme-success` | Added lines, accept mode, terminal green |
| `--theme-warning` | Waiting status, terminal yellow |
| `--theme-pro` | Auto mode, terminal magenta |

Leave one out and everything it would paint keeps its built-in colour. A
token or `--ansi-*` colour the file sets itself still wins.

## Looks: a backdrop behind glass

A theme file can paint one picture under the whole window. The sidebar, every
terminal pane and every thread then turn to glass over it, so the picture runs
unbroken behind all of them. The picture can move.

```css
:root {
  --theme-kind: dark;
  --theme-background: #070b1a;
  --theme-text: #dce4f7;
  --theme-accent: #7ce8c4;
  --theme-backdrop: linear-gradient(180deg, #040714, #14345a);
}
```

Setting `--theme-backdrop`, `--theme-scene` or `--theme-shader` is what turns
a theme into a look. In Settings › Appearance, a look that moves leads the
theme picker under **Live backgrounds** with a small play mark on its swatch,
so the moving ones are easy to find: one with a shader, a scene with CSS or SMIL animation in it, or an
`animation` on the backdrop or its layers. A still picture gets no mark.
Interface reads this from the file's text and shows the mark when it cannot
tell. The **Motion** menu in the Live backgrounds block sets how they move:
On, Still on battery or Off. Three finished examples are in
[docs/theme-examples](theme-examples): **Aurora** (dark), **Paper Tide**
(light) and **Neon Grid** (an SVG sky over a CSS floor). TN Code's themes are
ported beside them in [tn-code](theme-examples/tn-code): **TN Code**,
**Orbit** and **Glyph** move a shader pattern behind the window, and fifteen
more are plain palettes. Copy one into the themes folder to try it, or to
start your own from.

| Key | What it does | Default |
| --- | --- | --- |
| `--theme-backdrop` | Any CSS background: a colour, gradients, a `data:` image | `none` |
| `--theme-scene` | A picture drawn over it, sized to cover the window | `none` |
| `--theme-shader` | A fragment shader drawn under the scene (see [Shaders](#shaders)) | `none` |
| `--theme-backdrop-veil` | Any CSS background drawn over the art, to calm it | `none` |
| `--theme-backdrop-blur` | Blurs the art itself, for example `24px` | `0px` |
| `--theme-glass` | How far the backdrop shows through panels | `26%` |
| `--theme-glass-screen` | How far it shows through terminal screens | `14%` |
| `--theme-glass-blur` | Frosts the panels. Costly over a moving backdrop | `0px` |

Panels are the sidebar, pane frames and heads, a thread's page, and pages
whose words would otherwise sit on the bare picture (an empty surface, the
new-thread page, the launcher). Settings, menus, dialogs and the message box
stay solid. Raise the two glass values for a clearer picture, lower them for
calmer text.

The sidebar is frosted glass: behind it the picture is blurred to soft colour,
so no line or shape in it runs behind a name. It shows through as far as
`--theme-glass` says, and never more than 44%. The blur is made from a second,
still copy of the picture under the sidebar, which catches up with the moving
one twice a second (once a second on the battery), so the frost costs nothing
extra to draw. A file's own rules reach the copy by the same class names;
nothing needs writing for it. Stowed and brought back over the work, the
sidebar is solid.

Over a backdrop the pane you have selected stays at full strength and every
other pane's head and screen fade to 71%, the way Rex fades its inactive
panes, so the selected pane stands out from a moving picture. A pane that is
waiting on you does not fade. Built-in themes and theme files without a
backdrop do not fade their panes.

### Scenes

`--theme-scene` takes a picture as a `data:` URL. A PNG or JPEG is drawn as it
is. An SVG is drawn as real SVG, so everything in it can move: animate it with
CSS in its own `<style>`. (`<animate>`, `<animateTransform>` and
`<animateMotion>` run too, but they draw their shape again on every frame; see
[what motion costs](#motion-and-what-it-costs).)

Keep the SVG as its own file while you work on it, and embed it with the
helper beside the examples:

    python3 docs/theme-examples/embed-scene.py ~/.config/interface/themes/Mine.css scene.svg

It writes the `--theme-scene` line into the theme file. Interface repaints
when the theme file is saved, so run it again after each change to the SVG.

Give the SVG a `viewBox`. It fills the window like a photo cropped to fit
(`preserveAspectRatio="xMidYMid slice"` unless you set another), so keep what
matters away from the edges, and away from the middle where words sit. A scene
draws and nothing else: scripts, links, embedded pages and event handlers are
removed, and a reference may only point inside the scene (`#id`) or at an
embedded `data:image`.

### Shaders

`--theme-shader` takes a GLSL fragment shader (WebGL2, `#version 300 es`) as
a `data:` URL, and draws it on one canvas behind the scene. It is how TN Code
draws Orbit's dithered horizon and Glyph's field of dots, and suits any
pattern worked out per pixel: noise, waves, dither, a treated photograph.

    python3 docs/theme-examples/embed-scene.py ~/.config/interface/themes/Mine.css pattern.frag

| Key | What it does | Default |
| --- | --- | --- |
| `--theme-shader` | The shader: `url("data:x-shader/x-fragment;base64,…")` | `none` |
| `--theme-shader-scale` | Canvas pixels per point of the window, `0.1` to `2` | `0.5` |
| `--theme-shader-rate` | Frames a second, `10` to `30` | `20` |
| `--theme-shader-start` | The second its pattern opens on, so every launch looks the same | `0` |
| `--theme-shader-image` | A PNG, JPEG, WebP or GIF `data:` picture for the shader to read | `none` |

The shader is given `uniform vec2 u_resolution` (the canvas in its own
pixels), `uniform float u_time` (seconds the pattern has run) and, when the
file embeds a picture, `uniform sampler2D u_image` with
`uniform vec2 u_imageSize`. It writes one `out vec4` colour, premultiplied by
its alpha; where it is see-through, `--theme-backdrop` shows.

It costs little because it is drawn small and stretched with hard pixels: one
canvas pixel covers `1 / scale` points, so a dither or a dot grid sets its own
grain. TN Code's Orbit runs at `0.4` and twenty frames a second, Glyph at `1`
and fifteen; the canvas never exceeds about four megapixels. It moves and
rests exactly as every other backdrop does, and on the battery is drawn at
most fifteen times a second. A shader that does not compile leaves the plain
backdrop showing.

[tn-code/shaders](theme-examples/tn-code/shaders) holds TN Code's four:
`orbit.frag`, `glyph.frag`, and `picture-dither.frag` and `picture-dots.frag`,
which give any picture Orbit's or Glyph's treatment:

    python3 docs/theme-examples/embed-scene.py Mine.css docs/theme-examples/tn-code/shaders/picture-dither.frag
    python3 docs/theme-examples/embed-scene.py Mine.css photo.jpg --shader-image

### Layers

Above the scene sit six empty layers for your own CSS, each the size of the
window:

```css
.theme-backdrop-layer[data-layer="1"] {
  inset: -15% -10% 30% -10%;
  background: radial-gradient(40% 60% at 30% 45%, #35f0b08c, transparent 70%);
  filter: blur(36px);
  animation: sway 26s ease-in-out infinite alternate;
}
@keyframes sway {
  from { transform: translate3d(-5%, 0, 0) skewX(-7deg); }
  to { transform: translate3d(6%, 3%, 0) skewX(8deg); }
}
```

The order is the backdrop, the shader, the scene, layers 1 to 6, then the
veil. Change it with `z-index` (`.theme-backdrop-scene { z-index: 1; }` puts
the scene in front of the layers). These class names are a stable part of
theme files: `.theme-backdrop`, `.theme-backdrop-art`,
`.theme-backdrop-shader`, `.theme-backdrop-scene`, `.theme-backdrop-layer`
and `.theme-backdrop-veil`. The sidebar's frost draws the same art a second
time inside `.theme-backdrop-frost`: style the art by these names and the copy
follows; leave the frost's own box alone.

### Motion and what it costs

A moving backdrop is drawn thirty times a second, and fifteen on the battery.
So that it never costs anything nobody sees, the motion rests:

- whenever Interface is not the app in front, or its window is not being
  drawn;
- under macOS **Reduce motion**;
- on the battery, once nothing has been typed, clicked or scrolled for a
  minute (any input starts it again);
- on the battery altogether, when **Settings › Appearance › Live backgrounds ›
  Motion** is Still on battery;
- when that **Motion** menu is Off.

Resting holds the picture where it is. Under macOS **Reduce transparency**
panels and screens go solid, and the backdrop stays in the gutters.

What a look costs while it moves is decided by how it is drawn. Moving or
fading a finished picture costs almost nothing. Anything that has to be drawn
again on every frame costs several times more: measured on 2026-11-15, the
three examples went from about a quarter of one processor core (24 to 27%)
to 12 to 16% once nothing in them was drawn again, and 9 to 11% on the
battery, against 3 to 4% for the same window standing still, which is what a
built-in theme uses. (Hidden test window, two terminal panes open; the
figures compare with each other, not with a real display.) The rules that got
them there:

- **Animate only `transform` and `opacity`**, with CSS `@keyframes`: move,
  turn and fade. Colours, gradients, `background-position`, a path's `d` and
  SMIL animation are drawn again on every frame.
- **Never put `filter` on something that moves.** A blur on a moving layer was
  half of Aurora's cost. Paint the soft picture on a still child, the layer's
  `::before`, and animate the layer: it is blurred once and carried along.
- **No `mix-blend-mode`.** It makes the window put the whole backdrop together
  a second time on every redraw, moving or not. Use see-through colours.
- **Nothing moving under a mask or a clip.** Fade an edge with a still
  gradient laid over it (`::after`) instead of `mask-image`; keep what is
  inside an SVG `<mask>`, `<clipPath>` or `<pattern>` still.
- **Fade one shape, not a group.** A fading `<g>` of scattered shapes is put
  together separately, at its full size, on every frame. Draw a field of
  stars as one `<path>` with many dots in its `d`; the Aurora example's 130
  stars twinkle as six such paths.
- **Do not grow or shrink things in a scene**; pulse with `opacity`. A
  backdrop layer may scale if its rule has `will-change: transform`.
- **Leave `--theme-glass-blur` and `--theme-backdrop-blur` at `0px`** over a
  moving backdrop: they blur every panel, or the whole picture, again on
  every redraw.
- Each animation costs a little on every frame, so a few dozen is plenty.

The checker beside the examples reads a theme file, with its scene, and names
everything in it that breaks these rules, each with its fix:

    node docs/theme-examples/check-theme.mjs ~/.config/interface/themes/Mine.css

The three examples pass it, and each shows the patterns above.

### Ask an agent for one

The quickest way to get a look is to ask for it. In any pane or thread, say
what you want ("make me an Interface theme of a rainy city at night") and
point the agent at this file. The `interface-theme` skill in this repository
(`skills/interface-theme`) gives Claude Code and Codex the steps; install it
with `scripts/install-theme-skill.sh`.

## Start from another app's scheme

`iface theme convert` turns a colour scheme from another terminal or editor
into a theme file. It prints the CSS; redirect it into the folder:

    iface theme convert Dracula.itermcolors > ~/.config/interface/themes/Dracula.css

It reads:

| Format | Files | Colours it uses |
| --- | --- | --- |
| `iterm2` | `.itermcolors` (XML property list) | Background, Foreground, Ansi 0–15 |
| `ghostty` | Ghostty themes | `background`, `foreground`, `palette = N=…` |
| `alacritty` | Alacritty `.toml` | `[colors.primary]`, `[colors.normal]`, `[colors.bright]` |
| `base16` | Base16 or Base24 `.yaml` | `base00`–`base0F` (base16-shell's ANSI mapping), `variant` |
| `vscode` | VS Code colour theme `.json` (comments allowed) | `type`, `editor.*`, `sideBar.background`, `button.background`, `terminal.*` |

The format is detected from the file; `--format <name>` overrides it. The
scheme's background and text become `--theme-background`, `--theme-text` and
the terminal screen, its blue (or the editor's button colour) becomes
`--theme-accent`, and its ANSI colours become `--ansi-*`. Light or dark comes
from the file when it says, otherwise from the background. Everything else
falls back to the defaults in the key colour table above, so the result is a
starting point to edit.
Binary property lists need `plutil -convert xml1 <file>` first.

## Any token, and any CSS

A theme file is applied as written, after every stylesheet in the app. The
app's own colour tokens sit in a cascade layer (`interface-palette`), so a
plain `:root` rule in your file always wins over them:

```css
:root {
  --theme-kind: light;
  --theme-background: #f6f1e7;
  --accent: #8a3b1c;                 /* overrides what --theme-accent derives */
  --status-working: #b5562a;
  --font-ui: "Avenir Next", var(--font-ui-sans);
  --font-terminal: "Berkeley Mono", monospace;
}

.sidebar { letter-spacing: 0.01em; }  /* anything else is ordinary CSS */
```

Ordinary selectors compete with the app's rules by specificity, as in any
stylesheet. Selectors and layout other than the tokens below are not a stable
interface: an update can rename a class and leave such a rule matching
nothing.

### Colour tokens

Surfaces: `--editor-bg`, `--chrome-bg`, `--surface`, `--surface-raised`,
`--workbench-canvas`, `--workbench-panel`, `--dropdown-bg`, `--widget-bg`,
`--menu-bg`, `--input-bg`, `--backdrop`.

Text: `--fg`, `--fg-strong`, `--fg-muted`, `--fg-dim`, `--fg-faint`,
`--fg-selected-secondary`, `--input-fg`, `--input-placeholder`.

Lines and rows: `--border`, `--border-strong`, `--input-border`,
`--selection-border`, `--row-hover`, `--row-selected`, `--row-selected-blur`,
`--sidebar-selected-fill`, `--sidebar-selected-edge`, `--sidebar-parent-fill`,
`--sidebar-parent-hover`, `--tree-guide`, `--scrollbar`, `--scrollbar-active`,
`--selection`.

Accent and focus: `--accent`, `--accent-text`, `--accent-text-hover`,
`--accent-hover`, `--accent-fg` (text on the accent), `--focus`,
`--focus-contrast`, `--selection-accent`, `--badge-bg`, `--badge-fg`.

Status lights: `--status-working`, `--status-waiting`, `--status-failed`,
`--status-done`, `--status-idle`, `--danger-fg`, `--danger-bg`.

Diffs and access modes: `--diff-added`, `--diff-removed`, `--diff-added-bg`,
`--diff-removed-bg`, `--mode-plan`, `--mode-accept`, `--mode-danger`,
`--mode-auto`.

Terminal panes: `--terminal-canvas`, `--terminal-content-bg`,
`--terminal-header-bg`, `--terminal-header-bg-focused`,
`--terminal-header-border`, `--terminal-header-border-focused`,
`--terminal-tile-edge`, `--terminal-focus-edge`, `--terminal-fg`,
`--terminal-fg-strong`, `--terminal-fg-muted`, `--terminal-fg-dim`,
`--terminal-fg-faint`, `--terminal-border`, `--terminal-scrollbar`,
`--terminal-scrollbar-active`, `--terminal-drop-bg`, and
`--terminal-color-scheme` (`light` or `dark`: the screen's own scheme).

Shadows: `--overlay-shadow-color`, `--overlay-shadow-soft-color`,
`--composer-lift-color`.

Meaning and looks: `--theme-danger`, `--theme-success`, `--theme-warning`,
`--theme-pro`, `--theme-backdrop`, `--theme-scene`, `--theme-shader`,
`--theme-shader-image`, `--theme-shader-scale`, `--theme-shader-rate`,
`--theme-shader-start`, `--theme-backdrop-veil`, `--theme-backdrop-blur`,
`--theme-glass`, `--theme-glass-screen`, `--theme-glass-blur` (see the
sections above).

### Terminal colours

Set any of the 16 ANSI colours:

```css
:root {
  --ansi-black: #2a322b;   --ansi-bright-black: #6d786b;
  --ansi-red: #f07e8c;     --ansi-bright-red: #ff9aa6;
  --ansi-green: #9fd4a8;   --ansi-bright-green: #b6e3bd;
  --ansi-yellow: #e8c15a;  --ansi-bright-yellow: #f2d27e;
  --ansi-blue: #7fb0ff;    --ansi-bright-blue: #9fc4ff;
  --ansi-magenta: #c9a3e8; --ansi-bright-magenta: #dabcf2;
  --ansi-cyan: #79cfc4;    --ansi-bright-cyan: #97ddd3;
  --ansi-white: #cdd6c9;   --ansi-bright-white: #f1f5ee;
}
```

Colours you leave out come from a built-in set made for the screen: a light
set when `--terminal-content-bg` is a light hex colour (or
`--terminal-color-scheme` is `light`), otherwise a dark one.

### Fonts

`--font-ui` (the interface face), `--font-terminal` (terminal panes) and
`--font-mono` (code in the interface) take a font-family list. Fonts must be
installed on this Mac or embedded in the file as a `data:` URL.

## What a theme file cannot do

- Load anything from the network. `url(https://…)`, remote fonts and images
  are refused by the window's content policy; `@import` is ignored.
- Run code in the window. A theme file is only CSS, and a scene is only a
  drawing: its scripts and event handlers are removed. A shader runs on the
  graphics card and can only colour its own canvas, from the time, the canvas
  size and the picture the file itself embeds.
- Keep an animation running while Interface is behind another app, or outrank
  the Motion menu (On, Still on battery, Off).

Contrast is not checked: a theme can make text hard to read. The built-in
themes and an empty theme file of either kind meet WCAG AA.

## Troubleshooting

- **The theme is greyed out in Settings.** Hover it for the reason: the file
  is larger than 1 MB, or not UTF-8 text.
- **The theme vanished.** Interface lists `<Name>.css` files (up to 64)
  directly in the folder. Names are at most 64 characters, cannot start with
  a dot and cannot contain `/`, `\` or `:`. A renamed or deleted file that
  was selected falls back to what Follow macOS would paint until it returns.
- **The backdrop does not move.** Check that Interface is the app in front,
  that Settings › Appearance › Live backgrounds › Motion is not Off, and that
  macOS Reduce motion is off. On the battery it moves at half pace and rests
  after a minute without input, or stays still if Motion is Still on battery.
- **The Mac runs warm or the battery drains with a look on.** Run
  `docs/theme-examples/check-theme.mjs` on the theme file and fix what it
  names, or set Motion to Still on battery.
- **The backdrop does not show.** The file must set `--theme-backdrop`,
  `--theme-scene` or `--theme-shader` to something other than `none`. Under
  macOS Reduce transparency it shows only between panels. A shader that does
  not compile is left out; the reason is in the window's console.
- **Text is hard to read over the picture.** Lower `--theme-glass` and
  `--theme-glass-screen`, or add a `--theme-backdrop-veil`.
- **A token does nothing.** Check the spelling against the lists above, and
  that the rule is not inside a selector that never matches.
