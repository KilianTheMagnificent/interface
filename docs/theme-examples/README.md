# Example looks

Three finished looks for Interface: copy a `.css` file into
`~/.config/interface/themes/` and pick it in Settings › Appearance.

| Look | Kind | What moves |
| --- | --- | --- |
| `Aurora.css` | dark | Curtains of light (CSS layers) over a starry mountain lake (SVG scene) |
| `Paper Tide.css` | light | Sun, waves, a boat, gulls and clouds (SVG scene) |
| `Neon Grid.css` | dark | A striped sun and wireframe mountains (SVG scene) over a rushing grid (CSS layer) |

Each scene's editable source is in `scenes/`. After changing one, embed it
again:

    python3 embed-scene.py Aurora.css scenes/aurora.svg

`embed-scene.py` also embeds a fragment shader (`.frag`) as `--theme-shader`
and, with `--shader-image`, a picture for the shader to read.

[tn-code](tn-code) holds the themes of TN Code, the owner's T3 Code fork,
ported to Interface theme files: its own TN Code, Orbit, Glyph and Apple, T3
Code's two defaults and its five built-in palettes, light and dark. TN Code,
Orbit and Glyph draw their moving patterns with the shaders in
`tn-code/shaders`.

Before using a look of your own, check that nothing in it is drawn again on
every frame, which is what drains a battery:

    node check-theme.mjs ~/.config/interface/themes/Mine.css

It names each costly thing and its fix; the three examples pass it.

`electron/src/renderer/theme-look.test.ts` holds each `.css` to its scene
source and to the checker. The format, and what motion costs, are described in
[Theme files](../THEMES.md).
