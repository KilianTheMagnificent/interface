---
name: interface-theme
description: Make or change a colour theme or an animated look for the Interface desktop app, a .css file in ~/.config/interface/themes. Use when the user asks for an Interface theme, look, backdrop or moving scene, or to restyle or recolour Interface.
---

Interface themes are plain CSS files in `~/.config/interface/themes/<Name>.css`.
Interface lists them in Settings › Appearance under **Yours** (or first,
under **Live backgrounds**, when the backdrop moves) and repaints
within a second of each save, so the user watches the theme change as you
edit. Invoke this skill as `/interface-theme` in Claude Code or
`$interface-theme` in Codex; the user's trailing text is the theme they want.

Read [the theme guide](references/themes.md) before writing anything: it lists
every key, token and class a theme file may use. The three examples in
`references/` (`Aurora.css`, `Paper Tide.css`, `Neon Grid.css`, with their
scenes under `references/scenes/`) are finished looks to start from.

## Steps

1. **Name it and check the folder.** One file, `<Name>.css`, at most 64
   characters, no `/`, `\` or `:`. If a theme of that name exists, the user
   is asking you to change it; otherwise never overwrite a theme you were not
   asked about. Create the folder if it is missing.
2. **Start with the key colours.** Set `--theme-kind` (`light` or `dark`) and
   the eight `--theme-*` keys, then the four meaning colours
   (`--theme-danger`, `--theme-success`, `--theme-warning`, `--theme-pro`).
   Tint every surface from the look's own hue and keep neighbours close.
   Words come first: aim for body text at 7:1 on the background and on the
   terminal screen, and keep danger a red, success a green, warning an amber.
3. **Add the backdrop only if the user wants a picture or motion.**
   `--theme-backdrop` takes any CSS background. For a drawn scene, write an
   SVG beside the theme as `scenes/<Name>.svg` (Interface ignores subfolders)
   and embed it:

       python3 <this skill>/references/embed-scene.py ~/.config/interface/themes/<Name>.css ~/.config/interface/themes/scenes/<Name>.svg

   Run that again after every change to the SVG. Give the SVG a `viewBox`,
   animate with CSS in its own `<style>`, and use no script, link or outside
   file: Interface removes them.
   For a pattern worked out per pixel (noise, waves, dither, dots) or a
   photograph given a treatment, use a shader instead: a WebGL2 fragment
   shader in a `.frag` file, embedded the same way as `--theme-shader`. The
   guide's Shaders section lists what it is given. `references/shaders/` holds
   TN Code's four; `picture-dither.frag` and `picture-dots.frag` turn any
   picture into a moving backdrop:

       python3 <this skill>/references/embed-scene.py <theme.css> <this skill>/references/shaders/picture-dither.frag
       python3 <this skill>/references/embed-scene.py <theme.css> <picture.jpg> --shader-image

   Keep `--theme-shader-scale` at 1 or less (0.4 to 0.5 for a dither).
4. **Make the motion cheap, then check it.** A moving backdrop is drawn up to
   30 times a second. Moving or fading a finished picture costs almost
   nothing; anything that has to be drawn again on every frame costs several
   times more and drains the user's battery. So:
   - animate only `transform` (move, turn) and `opacity` (fade), with CSS
     `@keyframes`. Not colours, gradients, `background-position`, `d`, sizes,
     and not SMIL (`<animate>` and friends);
   - never put `filter` on something that moves. Paint the soft picture on a
     still child (the layer's `::before`) and animate the layer;
   - no `mix-blend-mode`, no masks or clips over moving things, and leave
     `--theme-glass-blur` and `--theme-backdrop-blur` at `0px`. Use
     see-through colours and still gradients instead;
   - fade one shape, not a group: draw a field of stars as one `<path>` with
     many dots in its `d`, and give each twinkling set its own path;
   - do not grow or shrink things in a scene; pulse with `opacity`. A backdrop
     layer may scale if its rule has `will-change: transform`.

   Then run the checker and fix everything it names, until it prints that
   nothing is drawn again on every frame:

       node <this skill>/references/check-theme.mjs ~/.config/interface/themes/<Name>.css

   The three examples pass it and show each pattern. Interface holds motion
   still whenever it is not the app in front, halves the pace on battery and
   rests it after a minute without input there; do not try to work around
   that.
5. **Tune the glass.** `--theme-glass` and `--theme-glass-screen` set how far
   the picture shows through panels and terminal screens. If words are hard
   to read over the picture, lower them or add a `--theme-backdrop-veil`.
6. **Hand over.** Tell the user the theme's name, that it is under **Yours**
   (**Live backgrounds** if it moves) in Settings › Appearance, and that it repaints as the file is saved. You
   cannot see their window: ask them to look in a terminal pane and in a
   thread, and adjust from what they report.

## Limits

A theme file is at most 1 MB of UTF-8 CSS. Nothing loads from the network
(`@import`, remote fonts and images are refused); embed pictures and fonts as
`data:` URLs. Selectors other than the documented `--` tokens and
`.theme-backdrop*` classes may stop matching after an update.
