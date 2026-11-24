# TN Code's themes

The themes of TN Code (the owner's T3 Code fork), as Interface theme files.
Copy any `.css` here into `~/.config/interface/themes/` and pick it in
Settings › Appearance.

| Theme | Kind | Backdrop |
| --- | --- | --- |
| `TN Code.css` | dark | Orbit's horizon |
| `TN Code Light.css` | light | plain, as in TN Code |
| `Orbit.css` | dark | Orbit's horizon: a dithered sunrise band that drifts and ripples |
| `Glyph.css` | dark | Glyph's lattice of dots, brightening in drifting bands |
| `Apple.css`, `Apple Dark.css` | light, dark | plain |
| `T3 Code.css`, `T3 Code Light.css` | dark, light | plain |
| `T3 Chat`, `Grove`, `Ocean`, `Ember`, `Iris` (each with a `Dark`) | light, dark | plain |

An Interface theme file is light or dark, so a TN Code theme with both sides
is two files. Interface works its colours out from eight keys, so each
palette's sixty roles come down to those keys, danger and warning, the border
and the text on the accent; where a palette keeps its accent for fills and it
is too dark to read as words, `--accent-text` takes its nearest readable
accent.

`shaders/` holds TN Code's four fragment shaders as it wrote them: `orbit`,
`glyph`, and `picture-dither` and `picture-dots`, which give any picture
Orbit's or Glyph's treatment ([Shaders](../../THEMES.md#shaders) says how to
use them in a theme of your own).

Every file here is written by `port-themes.mjs` from a TN Code checkout
(`~/.t3/projects/t3-apple` unless you pass another path). After TN Code's
themes change, run it again and copy the files you use:

    node port-themes.mjs
