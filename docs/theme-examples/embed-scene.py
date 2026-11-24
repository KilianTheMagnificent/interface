#!/usr/bin/env python3
"""Embed a scene, a shader or a shader's picture in a theme file.

    python3 embed-scene.py Aurora.css scenes/aurora.svg
    python3 embed-scene.py Mine.css pattern.frag
    python3 embed-scene.py Mine.css photo.jpg --shader-image

Rewrites one line of the theme (or adds it to the first `:root` rule) with the
file as a base64 data URL: an `.svg`, `.png`, `.jpg`, `.webp` or `.gif` becomes
`--theme-scene`, a `.frag` or `.glsl` fragment shader becomes `--theme-shader`,
and with `--shader-image` a picture becomes `--theme-shader-image`, the one a
shader reads as `u_image`. Interface repaints when the theme file is saved, so
run this after each change to the file. A theme file may be 1 MB at most.
"""
import base64, os, re, sys

TYPES = {
    ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".webp": "image/webp", ".gif": "image/gif", ".frag": "x-shader/x-fragment", ".glsl": "x-shader/x-fragment",
}

def main() -> int:
    arguments = [argument for argument in sys.argv[1:] if argument != "--shader-image"]
    shader_image = "--shader-image" in sys.argv[1:]
    if len(arguments) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    theme_path, source_path = arguments
    media = TYPES.get(os.path.splitext(source_path)[1].lower())
    if media is None:
        print(f"{source_path}: not an SVG, a picture or a .frag shader", file=sys.stderr)
        return 2
    if shader_image and not media.startswith("image/") or shader_image and media == "image/svg+xml":
        print("--shader-image takes a PNG, JPEG, WebP or GIF picture", file=sys.stderr)
        return 2
    token = "--theme-shader-image" if shader_image else (
        "--theme-shader" if media == "x-shader/x-fragment" else "--theme-scene")
    with open(source_path, "rb") as handle:
        encoded = base64.b64encode(handle.read()).decode("ascii")
    line = f'{token}: url("data:{media};base64,{encoded}");'
    with open(theme_path, encoding="utf-8") as handle:
        css = handle.read()
    # The value holds a `;` of its own (`data:image/svg+xml;base64,…`), so it
    # is matched as a whole quoted url(), never up to the first semicolon.
    value = re.escape(token) + r"""\s*:\s*(?:url\(\s*"[^"]*"\s*\)|url\(\s*'[^']*'\s*\)|[^;"']*)\s*;"""
    if re.search(value, css):
        css = re.sub(value, lambda _: line, css, count=1)
        # Tails an earlier version of this helper left behind after the line.
        css = re.sub(r"""(?<=\);)(?:base64,[A-Za-z0-9+/=]*"\);)+""", "", css)
    elif ":root" in css:
        css = re.sub(r"(:root\s*\{)", lambda m: m.group(1) + "\n  " + line, css, count=1)
    else:
        css = f":root {{\n  {line}\n}}\n" + css
    if len(css.encode("utf-8")) > 1_000_000:
        print(f"{theme_path} would be over 1 MB, which Interface refuses: use a smaller file "
              f"(for a picture: sips -Z 1600 {source_path} --out smaller.jpg)", file=sys.stderr)
        return 1
    with open(theme_path, "w", encoding="utf-8") as handle:
        handle.write(css)
    print(f"{theme_path}: {token} embedded ({len(encoded) // 1024 + 1} KB)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
