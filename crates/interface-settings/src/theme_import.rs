//! Converts other apps' colour schemes into Interface theme files.
//!
//! Reads an iTerm2 `.itermcolors` plist, a Ghostty theme, an Alacritty TOML
//! colour file, a Base16 YAML scheme or a VS Code colour theme, and writes the
//! CSS of an equivalent `~/.config/interface/themes/<name>.css` file. Nothing
//! here touches the filesystem: callers read the source and decide where the
//! CSS goes, so the theme folder stays the user's.
//!
//! Like `custom_themes`, these are deliberately small line and tag scanners
//! for the colour keys each format uses, not general parsers: a key this
//! module does not know is ignored, and a file with no colour it knows is
//! refused.

use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::sync::LazyLock;

use interface_contracts::appearance::ThemeKind;
use regex::Regex;

use crate::custom_themes::{CUSTOM_THEME_MAX_BYTES, relative_luminance};

/// The largest scheme file converted.
pub const SCHEME_IMPORT_MAX_BYTES: u64 = CUSTOM_THEME_MAX_BYTES;

/// The `--ansi-*` names, in ANSI index order 0–15.
const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/// A colour scheme format this module reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemeFormat {
    Iterm2,
    Ghostty,
    Alacritty,
    Base16,
    VsCode,
}

impl SchemeFormat {
    pub const ALL: [Self; 5] = [
        Self::Iterm2,
        Self::Ghostty,
        Self::Alacritty,
        Self::Base16,
        Self::VsCode,
    ];

    /// The spelling `--format` takes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iterm2 => "iterm2",
            Self::Ghostty => "ghostty",
            Self::Alacritty => "alacritty",
            Self::Base16 => "base16",
            Self::VsCode => "vscode",
        }
    }

    /// The format spelled exactly as [`Self::as_str`] writes it.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == value)
    }

    /// The name people know the source by.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Iterm2 => "iTerm2",
            Self::Ghostty => "Ghostty",
            Self::Alacritty => "Alacritty",
            Self::Base16 => "Base16",
            Self::VsCode => "VS Code",
        }
    }

    /// Recognises a scheme from its text. `None` when nothing matches.
    #[must_use]
    pub fn detect(text: &str) -> Option<Self> {
        let trimmed = text.trim_start_matches('\u{feff}').trim_start();
        if trimmed.starts_with("<?xml")
            || trimmed.starts_with("<!DOCTYPE plist")
            || trimmed.starts_with("<plist")
        {
            return Some(Self::Iterm2);
        }
        if strip_jsonc(trimmed).trim_start().starts_with('{') {
            return Some(Self::VsCode);
        }
        let lines = || text.lines().map(str::trim);
        if lines().any(|line| line.starts_with("[colors")) {
            return Some(Self::Alacritty);
        }
        if lines().any(|line| {
            line.trim_start_matches(['"', '\''])
                .strip_prefix("base00")
                .is_some_and(|rest| {
                    rest.trim_start_matches(['"', '\''])
                        .trim_start()
                        .starts_with(':')
                })
        }) {
            return Some(Self::Base16);
        }
        if lines().any(|line| {
            line.split_once('=').is_some_and(|(key, _)| {
                matches!(key.trim(), "palette" | "background" | "foreground")
            })
        }) {
            return Some(Self::Ghostty);
        }
        None
    }
}

impl fmt::Display for SchemeFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Why a scheme could not be converted.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SchemeImportError {
    #[error("The file is larger than 1 MB.")]
    TooLarge,
    #[error("The file is not UTF-8 text.")]
    NotText,
    #[error("The file is a binary property list. Convert it first: plutil -convert xml1 <file>")]
    BinaryPlist,
    #[error(
        "The format was not recognised. Pass --format with one of: iterm2, ghostty, alacritty, base16, vscode."
    )]
    UnknownFormat,
    #[error("The file is not valid {0} JSON: {1}")]
    InvalidJson(SchemeFormat, String),
    #[error("No colours were found in the {0} file.")]
    NoColours(SchemeFormat),
}

/// A converted scheme: the theme file's text and what was learnt making it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedScheme {
    pub format: SchemeFormat,
    pub kind: ThemeKind,
    /// The scheme's own name, when the file carries one (VS Code, Base16).
    pub name: Option<String>,
    pub css: String,
}

/// Converts a scheme file's bytes into theme-file CSS. `format` overrides
/// detection; `source` names the file in the CSS header comment.
///
/// # Errors
///
/// Returns why the file cannot be converted: too large, not text, an
/// unrecognised format, or no colours this module knows.
pub fn convert_scheme(
    bytes: &[u8],
    format: Option<SchemeFormat>,
    source: &str,
) -> Result<ImportedScheme, SchemeImportError> {
    if bytes.len() as u64 > SCHEME_IMPORT_MAX_BYTES {
        return Err(SchemeImportError::TooLarge);
    }
    if bytes.starts_with(b"bplist") {
        return Err(SchemeImportError::BinaryPlist);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SchemeImportError::NotText)?;
    let text = text.trim_start_matches('\u{feff}');
    let format = format
        .or_else(|| SchemeFormat::detect(text))
        .ok_or(SchemeImportError::UnknownFormat)?;
    let palette = match format {
        SchemeFormat::Iterm2 => iterm2(text),
        SchemeFormat::Ghostty => ghostty(text),
        SchemeFormat::Alacritty => alacritty(text),
        SchemeFormat::Base16 => base16(text),
        SchemeFormat::VsCode => vscode(text)?,
    };
    if palette.is_empty() {
        return Err(SchemeImportError::NoColours(format));
    }
    let kind = palette.kind();
    let css = palette.css(format, kind, source);
    Ok(ImportedScheme {
        format,
        kind,
        name: palette.name.clone(),
        css,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    /// `#rgb`, `#rrggbb`, `#rrggbbaa`, `0xrrggbb` or bare `rrggbb`. Alpha is
    /// dropped: theme keys are opaque colours.
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim().trim_matches(['"', '\'']).trim();
        let digits = value
            .strip_prefix('#')
            .or_else(|| value.strip_prefix("0x"))
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
        match digits.len() {
            3 => {
                let nibble = |i: usize| {
                    let n = u8::from_str_radix(digits.get(i..=i)?, 16).ok()?;
                    Some(n * 17)
                };
                Some(Self(nibble(0)?, nibble(1)?, nibble(2)?))
            }
            6 | 8 => Some(Self(byte(0)?, byte(2)?, byte(4)?)),
            _ => None,
        }
    }

    fn from_unit(red: f64, green: f64, blue: f64) -> Self {
        let channel = |value: f64| {
            let value = if value.is_finite() { value } else { 0.0 };
            // In range 0..=255 after the clamp, so the cast cannot truncate.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            byte
        };
        Self(channel(red), channel(green), channel(blue))
    }

    /// `self` moved `amount` (0–1) of the way to `other`.
    fn mix(self, other: Self, amount: f64) -> Self {
        let channel = |a: u8, b: u8| {
            let value = f64::from(a) + (f64::from(b) - f64::from(a)) * amount;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = value.round().clamp(0.0, 255.0) as u8;
            byte
        };
        Self(
            channel(self.0, other.0),
            channel(self.1, other.1),
            channel(self.2, other.2),
        )
    }

    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// The colours a scheme gave, in theme-file terms.
#[derive(Clone, Debug, Default)]
struct Palette {
    name: Option<String>,
    kind: Option<ThemeKind>,
    background: Option<Rgb>,
    foreground: Option<Rgb>,
    panel: Option<Rgb>,
    raised: Option<Rgb>,
    accent: Option<Rgb>,
    screen: Option<Rgb>,
    screen_text: Option<Rgb>,
    ansi: [Option<Rgb>; 16],
}

impl Palette {
    fn is_empty(&self) -> bool {
        [
            self.background,
            self.foreground,
            self.panel,
            self.raised,
            self.accent,
            self.screen,
            self.screen_text,
        ]
        .iter()
        .chain(&self.ansi)
        .all(Option::is_none)
    }

    fn kind(&self) -> ThemeKind {
        self.kind
            .or_else(|| {
                let background = self.background.or(self.screen)?;
                let luminance = relative_luminance(&background.hex())?;
                Some(if luminance > 0.4 {
                    ThemeKind::Light
                } else {
                    ThemeKind::Dark
                })
            })
            .unwrap_or(ThemeKind::Dark)
    }

    fn css(&self, format: SchemeFormat, kind: ThemeKind, source: &str) -> String {
        let background = self.background.or(self.screen);
        let foreground = self.foreground.or(self.screen_text);
        // Cards and menus sit one step off the canvas, toward the text.
        let raised = self.raised.or_else(|| {
            let (background, foreground) = (background?, foreground?);
            Some(background.mix(foreground, 0.08))
        });
        let accent = self.accent.or(self.ansi[4]).or(self.ansi[12]);
        let mut css = String::new();
        let title = comment_text(self.name.as_deref().unwrap_or(source));
        let _ = writeln!(css, "/* {title}");
        let _ = writeln!(
            css,
            "   Converted from {} by `iface theme convert`{}.",
            format.label(),
            if title == comment_text(source) {
                String::new()
            } else {
                format!(" ({})", comment_text(source))
            }
        );
        let _ = writeln!(
            css,
            "   Edit freely: every token is described in docs/THEMES.md. */"
        );
        css.push_str(":root {\n");
        let kind = match kind {
            ThemeKind::Light => "light",
            ThemeKind::Dark => "dark",
        };
        let _ = writeln!(css, "  --theme-kind: {kind};");
        let keys = [
            ("--theme-background", background),
            ("--theme-panel", self.panel),
            ("--theme-raised", raised),
            ("--theme-text", foreground),
            ("--theme-accent", accent),
            ("--theme-screen", self.screen.or(background)),
            ("--theme-screen-text", self.screen_text.or(foreground)),
        ];
        for (property, colour) in keys {
            if let Some(colour) = colour {
                let _ = writeln!(css, "  {property}: {};", colour.hex());
            }
        }
        if self.ansi.iter().any(Option::is_some) {
            css.push('\n');
            for (name, colour) in ANSI_NAMES.iter().zip(&self.ansi) {
                if let Some(colour) = colour {
                    let _ = writeln!(css, "  --ansi-{name}: {};", colour.hex());
                }
            }
        }
        css.push_str("}\n");
        css
    }
}

/// Text safe inside a CSS comment and a terminal: printable, one line, no
/// comment terminator, bounded.
fn comment_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .replace("*/", "* /");
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut bounded: String = cleaned.chars().take(80).collect();
    if bounded.is_empty() {
        bounded.push_str("Imported theme");
    }
    bounded
}

/// A `<key>` followed by its `<dict>`: one iTerm2 colour entry.
static PLIST_ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<key>\s*([^<]+?)\s*</key>\s*<dict>(.*?)</dict>").expect("static pattern")
});
static PLIST_COMPONENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<key>\s*(Red|Green|Blue) Component\s*</key>\s*<(?:real|integer)>\s*([^<]+?)\s*</",
    )
    .expect("static pattern")
});

fn iterm2(text: &str) -> Palette {
    let mut palette = Palette::default();
    for captures in PLIST_ENTRY.captures_iter(text) {
        let mut channels = [None; 3];
        for part in PLIST_COMPONENT.captures_iter(&captures[2]) {
            let index = match &part[1] {
                "Red" => 0,
                "Green" => 1,
                _ => 2,
            };
            channels[index] = part[2].parse::<f64>().ok();
        }
        let [Some(red), Some(green), Some(blue)] = channels else {
            continue;
        };
        let colour = Some(Rgb::from_unit(red, green, blue));
        match captures[1].trim() {
            "Background Color" => palette.background = colour,
            "Foreground Color" => palette.foreground = colour,
            key => {
                if let Some(index) = key
                    .strip_prefix("Ansi ")
                    .and_then(|rest| rest.strip_suffix(" Color"))
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| *n < 16)
                {
                    palette.ansi[index] = colour;
                }
            }
        }
    }
    palette
}

fn ghostty(text: &str) -> Palette {
    let mut palette = Palette::default();
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "background" => palette.background = Rgb::parse(value),
            "foreground" => palette.foreground = Rgb::parse(value),
            "palette" => {
                if let Some((index, colour)) = value.split_once('=')
                    && let Ok(index) = index.trim().parse::<usize>()
                    && index < 16
                {
                    palette.ansi[index] = Rgb::parse(colour);
                }
            }
            _ => {}
        }
    }
    palette
}

/// Alacritty's `[colors.*]` tables, including dotted keys and inline tables.
fn alacritty(text: &str) -> Palette {
    let mut values: HashMap<String, String> = HashMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = toml_line(line);
        if let Some(header) = line.strip_prefix('[') {
            header
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .clone_into(&mut section);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = join_key(&section, key.trim());
        let value = value.trim();
        if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
            for pair in inner.split(',') {
                if let Some((inner_key, inner_value)) = pair.split_once('=') {
                    values.insert(
                        join_key(&key, inner_key.trim()),
                        inner_value.trim().to_owned(),
                    );
                }
            }
        } else {
            values.insert(key, value.to_owned());
        }
    }
    let colour = |key: &str| values.get(key).and_then(|value| Rgb::parse(value));
    let mut palette = Palette {
        background: colour("colors.primary.background"),
        foreground: colour("colors.primary.foreground"),
        ..Palette::default()
    };
    for (index, name) in ANSI_NAMES[..8].iter().enumerate() {
        palette.ansi[index] = colour(&format!("colors.normal.{name}"));
        palette.ansi[index + 8] = colour(&format!("colors.bright.{name}"));
    }
    palette
}

/// A TOML line without its comment. `#` inside a quoted value is kept.
fn toml_line(line: &str) -> &str {
    let mut quote = None;
    for (index, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') => return line[..index].trim(),
            _ => {}
        }
    }
    line.trim()
}

fn join_key(section: &str, key: &str) -> String {
    let key = key.trim_matches(['"', '\'']);
    if section.is_empty() {
        key.to_owned()
    } else {
        format!("{section}.{key}")
    }
}

// The base16-shell mapping from scheme slots to ANSI colours.
const BASE16_ANSI_SLOTS: [usize; 16] = [
    0x0, 0x8, 0xB, 0xA, 0xD, 0xE, 0xC, 0x5, 0x3, 0x8, 0xB, 0xA, 0xD, 0xE, 0xC, 0x7,
];

/// Base16 and Base24 YAML, flat (`base00: "181818"`) or under `palette:`.
fn base16(text: &str) -> Palette {
    let mut bases: [Option<Rgb>; 16] = [None; 16];
    let mut name = None;
    let mut kind = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        let value = value.split(" #").next().unwrap_or(value).trim();
        let unquoted = value.trim_matches(['"', '\'']).trim();
        match key {
            "scheme" | "name" if name.is_none() && !unquoted.is_empty() => {
                name = Some(unquoted.to_owned());
            }
            "variant" => {
                kind = match unquoted {
                    "light" => Some(ThemeKind::Light),
                    "dark" => Some(ThemeKind::Dark),
                    _ => kind,
                };
            }
            _ => {
                if let Some(index) = key
                    .strip_prefix("base0")
                    .filter(|digit| digit.len() == 1)
                    .and_then(|digit| usize::from_str_radix(digit, 16).ok())
                {
                    bases[index] = Rgb::parse(unquoted);
                }
            }
        }
    }
    let mut palette = Palette {
        name,
        kind,
        background: bases[0x0],
        foreground: bases[0x5],
        raised: bases[0x1],
        accent: bases[0xD],
        ..Palette::default()
    };
    for (index, slot) in BASE16_ANSI_SLOTS.iter().enumerate() {
        palette.ansi[index] = bases[*slot];
    }
    palette
}

/// VS Code's terminal colour keys, in ANSI index order.
const VSCODE_ANSI_KEYS: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

fn vscode(text: &str) -> Result<Palette, SchemeImportError> {
    let json: serde_json::Value = serde_json::from_str(&strip_jsonc(text))
        .map_err(|error| SchemeImportError::InvalidJson(SchemeFormat::VsCode, error.to_string()))?;
    let colours = json.get("colors").and_then(serde_json::Value::as_object);
    let colour = |key: &str| {
        colours
            .and_then(|colours| colours.get(key))
            .and_then(serde_json::Value::as_str)
            .and_then(Rgb::parse)
    };
    let first = |keys: &[&str]| keys.iter().find_map(|key| colour(key));
    let kind = match json.get("type").and_then(serde_json::Value::as_str) {
        Some("light" | "hcLight") => Some(ThemeKind::Light),
        Some("dark" | "hc" | "hcDark") => Some(ThemeKind::Dark),
        _ => None,
    };
    let mut palette = Palette {
        name: json
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned),
        kind,
        background: colour("editor.background"),
        foreground: first(&["editor.foreground", "foreground"]),
        panel: colour("sideBar.background"),
        raised: first(&[
            "editorWidget.background",
            "dropdown.background",
            "menu.background",
        ]),
        accent: first(&["button.background", "focusBorder", "textLink.foreground"]),
        screen: colour("terminal.background"),
        screen_text: colour("terminal.foreground"),
        ..Palette::default()
    };
    for (index, key) in VSCODE_ANSI_KEYS.iter().enumerate() {
        palette.ansi[index] = colour(key);
    }
    Ok(palette)
}

/// JSON with comments and trailing commas (VS Code's `jsonc`) as plain JSON.
/// Strings, including ones holding `//`, are copied untouched.
fn strip_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    let mut start = 0;
    let mut pending_comma: Option<usize> = None;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
                pending_comma = None;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                out.push_str(&text[start..index]);
                index = text[index..]
                    .find('\n')
                    .map_or(bytes.len(), |end| index + end);
                start = index;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                out.push_str(&text[start..index]);
                index = text[index + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| index + 2 + end + 2);
                out.push(' ');
                start = index;
            }
            b',' => {
                out.push_str(&text[start..index]);
                pending_comma = Some(out.len());
                out.push(',');
                index += 1;
                start = index;
            }
            b'}' | b']' => {
                if let Some(at) = pending_comma.take() {
                    out.push_str(&text[start..index]);
                    out.replace_range(at..=at, " ");
                    start = index;
                }
                index += 1;
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            _ => {
                pending_comma = None;
                index += 1;
            }
        }
    }
    out.push_str(&text[start.min(text.len())..]);
    out
}

#[cfg(test)]
#[path = "theme_import_tests.rs"]
mod tests;
