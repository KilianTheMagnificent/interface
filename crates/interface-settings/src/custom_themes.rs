//! Theme files the user keeps in `~/.config/interface/themes`.
//!
//! Each `<name>.css` file is a theme. The renderer applies its CSS as written;
//! this module only finds the files, reads them within bounds and pulls out
//! the few facts the host and main process need before any CSS runs: whether
//! the theme is light or dark, its window fill and its Settings swatch.
//!
//! The folder belongs to the user, not to the app: it may be a link into a
//! dotfiles repository, nothing here ever writes a theme file, and a file that
//! cannot be read is reported rather than failing the appearance read.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use interface_contracts::appearance::{CustomThemeName, CustomThemeSummary, ThemeId, ThemeKind};

/// The largest theme file read. Obsidian-sized themes with embedded fonts
/// stay well inside it.
pub const CUSTOM_THEME_MAX_BYTES: u64 = 1024 * 1024;
/// The most theme files listed; further files are ignored by name order.
pub const CUSTOM_THEME_MAX_COUNT: usize = 64;
/// The longest declared value kept for a swatch.
const PREVIEW_VALUE_MAX_CHARS: usize = 128;

/// Custom properties a Settings swatch may draw from, in addition to
/// `--theme-kind`. The renderer picks per swatch part.
pub const CUSTOM_THEME_PREVIEW_PROPERTIES: [&str; 14] = [
    "--theme-background",
    "--theme-panel",
    "--theme-text",
    "--theme-accent",
    "--theme-screen",
    "--theme-screen-text",
    "--editor-bg",
    "--chrome-bg",
    "--terminal-header-bg",
    "--terminal-content-bg",
    "--terminal-fg",
    "--accent",
    "--status-working",
    "--status-done",
];

/// A swatch entry that is not a colour, for a file that paints a backdrop:
/// `moving` when something in it moves (a shader, a scene with CSS or SMIL
/// animation in it, or an `animation` on the backdrop or its layers), `still`
/// otherwise. Read from the text, erring toward `moving` when unsure.
/// Settings marks such themes in the picker, so the ones with a moving
/// backdrop can be told from plain palettes before one is chosen.
pub const CUSTOM_THEME_LOOK_PROPERTY: &str = "--theme-look";

/// The themes folder below a home directory.
#[must_use]
pub fn custom_themes_directory(home: &Path) -> PathBuf {
    home.join(".config").join("interface").join("themes")
}

/// A theme file's text with what was learnt from it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomThemeFile {
    pub summary: CustomThemeSummary,
    pub css: Option<String>,
}

#[derive(Clone)]
struct Cached {
    len: u64,
    modified: Option<SystemTime>,
    file: CustomThemeFile,
}

/// Reads theme files, caching each by size and modification time so the
/// frequent appearance reads do not re-read unchanged files.
pub struct CustomThemeLibrary {
    directory: PathBuf,
    cache: Mutex<HashMap<CustomThemeName, Cached>>,
}

impl CustomThemeLibrary {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            cache: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Creates the folder so people can find it. Never changes an existing
    /// one, its permissions, or a link standing in its place.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error when the folder cannot be created.
    pub fn ensure_directory(&self) -> std::io::Result<()> {
        if fs::metadata(&self.directory).is_ok() {
            return Ok(());
        }
        fs::create_dir_all(&self.directory)
    }

    /// Every theme file, sorted by name without regard to case. A missing or
    /// unreadable folder lists nothing.
    #[must_use]
    pub fn list(&self) -> Vec<CustomThemeSummary> {
        self.names()
            .into_iter()
            .filter_map(|name| self.read(&name).map(|file| file.summary))
            .collect()
    }

    /// One theme file, read when it changed since the last read. `None` when
    /// there is no such regular file.
    #[must_use]
    pub fn read(&self, name: &CustomThemeName) -> Option<CustomThemeFile> {
        let path = self.directory.join(name.file_name());
        // Follows links on purpose: a theme may live in a dotfiles repository.
        let metadata = fs::metadata(&path).ok().filter(fs::Metadata::is_file)?;
        let modified = metadata.modified().ok();
        let mut cache = self.cache.lock().ok()?;
        if let Some(cached) = cache.get(name)
            && cached.len == metadata.len()
            && cached.modified == modified
            && modified.is_some()
        {
            return Some(cached.file.clone());
        }
        let file = load(name, &path, metadata.len());
        cache.insert(
            name.clone(),
            Cached {
                len: metadata.len(),
                modified,
                file: file.clone(),
            },
        );
        Some(file)
    }

    /// Whether `name` is a theme file that can be painted.
    #[must_use]
    pub fn is_paintable(&self, name: &CustomThemeName) -> bool {
        self.read(name).is_some_and(|file| file.css.is_some())
    }

    /// Changes whenever a theme file is added, removed, renamed or saved.
    /// Zero when the folder is missing.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.scan().0
    }

    /// [`Self::fingerprint`] and how many theme files the folder holds, from
    /// one listing.
    #[must_use]
    pub fn scan(&self) -> (u64, usize) {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return (0, 0);
        };
        let mut seen: Vec<(String, u64, u128)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                theme_name(&name)?;
                let metadata = fs::metadata(entry.path()).ok()?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |time| time.as_nanos());
                Some((name, metadata.len(), modified))
            })
            .collect();
        seen.sort();
        let themes = seen.len();
        let mut hash = Fnv::new();
        hash.write(b"themes");
        for (name, len, modified) in seen {
            hash.write(name.as_bytes());
            hash.write(&len.to_le_bytes());
            hash.write(&modified.to_le_bytes());
        }
        (hash.finish(), themes)
    }

    fn names(&self) -> Vec<CustomThemeName> {
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut names: Vec<CustomThemeName> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| theme_name(&entry.file_name().into_string().ok()?))
            .collect();
        names.sort_by(|a, b| {
            a.as_str()
                .to_lowercase()
                .cmp(&b.as_str().to_lowercase())
                .then_with(|| a.cmp(b))
        });
        names.truncate(CUSTOM_THEME_MAX_COUNT);
        names
    }
}

/// The theme a directory entry names, when it is a visible `.css` file.
fn theme_name(file_name: &str) -> Option<CustomThemeName> {
    let stem = file_name
        .strip_suffix(".css")
        .or_else(|| file_name.strip_suffix(".CSS"))?;
    CustomThemeName::new(stem).ok()
}

fn load(name: &CustomThemeName, path: &Path, len: u64) -> CustomThemeFile {
    let problem = |problem: &str, revision: String| CustomThemeFile {
        summary: CustomThemeSummary {
            id: ThemeId::Custom(name.clone()),
            name: name.as_str().to_owned(),
            kind: ThemeKind::Dark,
            background: None,
            revision,
            preview: BTreeMap::new(),
            problem: Some(problem.to_owned()),
        },
        css: None,
    };
    if len > CUSTOM_THEME_MAX_BYTES {
        return problem("The file is larger than 1 MB.", format!("size-{len}"));
    }
    let mut bytes = Vec::new();
    let read = fs::File::open(path).and_then(|file| {
        file.take(CUSTOM_THEME_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
    });
    if read.is_err() {
        return problem("The file could not be read.", "unreadable".to_owned());
    }
    let revision = revision(&bytes);
    if bytes.len() as u64 > CUSTOM_THEME_MAX_BYTES {
        return problem("The file is larger than 1 MB.", revision);
    }
    let Ok(css) = String::from_utf8(bytes) else {
        return problem("The file is not UTF-8 text.", revision);
    };
    let declared = declarations(&css);
    let background = ["--editor-bg", "--theme-background"]
        .iter()
        .find_map(|property| declared.get(*property).and_then(|value| hex_colour(value)));
    let kind = declared
        .get("--theme-kind")
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "light" => Some(ThemeKind::Light),
            "dark" => Some(ThemeKind::Dark),
            _ => None,
        })
        .or_else(|| {
            background
                .as_deref()
                .and_then(relative_luminance)
                .map(|luminance| {
                    if luminance > 0.4 {
                        ThemeKind::Light
                    } else {
                        ThemeKind::Dark
                    }
                })
        })
        .unwrap_or(ThemeKind::Dark);
    let mut preview: BTreeMap<String, String> = CUSTOM_THEME_PREVIEW_PROPERTIES
        .iter()
        .filter_map(|property| {
            let value = declared.get(*property)?;
            (value.chars().count() <= PREVIEW_VALUE_MAX_CHARS
                && !value.chars().any(char::is_control))
            .then(|| ((*property).to_owned(), value.clone()))
        })
        .collect();
    // A scene or a shader is far longer than a swatch value, so the look is
    // said in a word instead of being copied.
    let paints = |property: &str| {
        declared
            .get(property)
            .is_some_and(|value| !value.eq_ignore_ascii_case("none"))
    };
    if paints("--theme-backdrop") || paints("--theme-scene") || paints("--theme-shader") {
        let look = if look_moves(&css, &declared) {
            "moving"
        } else {
            "still"
        };
        preview.insert(CUSTOM_THEME_LOOK_PROPERTY.to_owned(), look.to_owned());
    }
    CustomThemeFile {
        summary: CustomThemeSummary {
            id: ThemeId::Custom(name.clone()),
            name: name.as_str().to_owned(),
            kind,
            background,
            revision,
            preview,
            problem: None,
        },
        css: Some(css),
    }
}

/// Whether a theme file that paints a backdrop moves: its shader, a scene with
/// anything animated in it, or an `animation` on the backdrop or its layers.
/// A plain reading of the text, not a CSS parser; when it cannot tell (a
/// picture it cannot read, a scene that does not decode) it says moving.
fn look_moves(css: &str, declared: &HashMap<String, String>) -> bool {
    let paints = |property: &str| {
        declared
            .get(property)
            .is_some_and(|value| !value.eq_ignore_ascii_case("none"))
    };
    paints("--theme-shader")
        || declared
            .get("--theme-scene")
            .is_some_and(|value| !value.eq_ignore_ascii_case("none") && scene_moves(value))
        || backdrop_animates(&strip_comments(css))
}

/// Whether a `--theme-scene` value can move. A CSS gradient or colour cannot,
/// nor can a JPEG; an SVG moves when it carries CSS or SMIL animation; any
/// other picture (an animated GIF, PNG or WebP) or anything unreadable may.
fn scene_moves(value: &str) -> bool {
    let value = value.trim();
    let Some(inner) = value
        .get(..4)
        .filter(|start| start.eq_ignore_ascii_case("url("))
        .and_then(|_| value[4..].strip_suffix(')'))
    else {
        return false;
    };
    let url = inner.trim().trim_matches(|c| c == '"' || c == '\'');
    let Some((head, payload)) = url
        .get(..5)
        .filter(|start| start.eq_ignore_ascii_case("data:"))
        .and_then(|_| url[5..].split_once(','))
    else {
        return true;
    };
    let mut parts = head.split(';');
    let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    if media == "image/jpeg" || media == "image/jpg" {
        return false;
    }
    if media != "image/svg+xml" {
        return true;
    }
    let bytes = if parts.any(|part| part.trim().eq_ignore_ascii_case("base64")) {
        let compact: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, compact) {
            Ok(bytes) => bytes,
            Err(_) => return true,
        }
    } else {
        percent_decode(payload)
    };
    let svg = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    !svg.contains("<svg") || svg_animates(&svg)
}

/// CSS animation (`animation`, `@keyframes`) or a SMIL animation element in
/// lower-cased SVG text.
fn svg_animates(svg: &str) -> bool {
    if svg.contains("animation") || svg.contains("@keyframes") {
        return true;
    }
    ["animate", "set"].iter().any(|tag| {
        svg.match_indices(tag).any(|(at, _)| {
            let before = svg[..at].chars().next_back();
            let after = svg[at + tag.len()..].chars().next();
            matches!(before, Some('<' | ':'))
                && (*tag == "animate"
                    || matches!(after, Some(c) if c.is_whitespace() || c == '/' || c == '>'))
        })
    })
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        if let (b'%', Some(decoded)) = (byte, hex) {
            out.push(decoded);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    out
}

/// Whether a rule that reaches the backdrop (`.theme-backdrop…`, a
/// `[data-layer]` or a universal selector) sets an `animation` other than
/// `none`. Quoted text is skipped, so a scene's own rules are not read here.
fn backdrop_animates(css: &str) -> bool {
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut segment = 0;
    let mut chars = css.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' | '\'' => {
                let mut escaped = false;
                for (_, next) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if next == '\\' {
                        escaped = true;
                    } else if next == c {
                        break;
                    }
                }
            }
            '{' => {
                open.push((segment, at + 1));
                segment = at + 1;
            }
            '}' => {
                if let Some((selector_start, body_start)) = open.pop() {
                    let selector = css[selector_start..body_start - 1].to_ascii_lowercase();
                    let reaches = selector.contains("theme-backdrop")
                        || selector.contains("data-layer")
                        || selector.contains('*');
                    if reaches && sets_animation(&css[body_start..at]) {
                        return true;
                    }
                }
                segment = at + 1;
            }
            ';' => segment = at + 1,
            _ => {}
        }
    }
    false
}

/// `animation` or `animation-name` declared with a value other than `none`.
fn sets_animation(body: &str) -> bool {
    body.split([';', '{', '}']).any(|declaration| {
        let Some((name, value)) = declaration.split_once(':') else {
            return false;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim();
        (name == "animation" || name == "animation-name")
            && !value.is_empty()
            && !value.eq_ignore_ascii_case("none")
    })
}

/// The first value given to each custom property anywhere in the file.
/// Deliberately not a CSS parser: it skips comments and strings and reads
/// `--name: value` up to the next `;` or `}` at the same nesting depth.
fn declarations(css: &str) -> HashMap<String, String> {
    let text = strip_comments(css);
    let bytes = text.as_bytes();
    let mut found = HashMap::new();
    let mut index = 0;
    while index + 2 < bytes.len() {
        let starts_property = bytes[index] == b'-'
            && bytes[index + 1] == b'-'
            && (index == 0
                || matches!(bytes[index - 1], b'{' | b';' | b' ' | b'\t' | b'\n' | b'\r'));
        if !starts_property {
            index += 1;
            continue;
        }
        let name_end = text[index..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map_or(bytes.len(), |offset| index + offset);
        let name = &text[index..name_end];
        let rest = text[name_end..].trim_start();
        let Some(value_text) = rest.strip_prefix(':') else {
            index = name_end;
            continue;
        };
        let value_start = text.len() - value_text.len();
        let mut depth = 0_i32;
        let mut end = text.len();
        for (offset, c) in value_text.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ';' | '}' if depth <= 0 => {
                    end = value_start + offset;
                    break;
                }
                _ => {}
            }
        }
        let value = text[value_start..end]
            .trim()
            .trim_end_matches("!important")
            .trim();
        if name.len() > 2 && !value.is_empty() {
            found
                .entry(name.to_owned())
                .or_insert_with(|| value.to_owned());
        }
        index = end.max(name_end);
    }
    found
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => {
                out.push(' ');
                rest = &rest[start + 2 + end + 2..];
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` as `#rrggbb`.
fn hex_colour(value: &str) -> Option<String> {
    let digits = value.strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digits = digits.to_ascii_lowercase();
    match digits.len() {
        3 => Some(digits.chars().fold(String::from("#"), |mut out, c| {
            out.push(c);
            out.push(c);
            out
        })),
        6 => Some(format!("#{digits}")),
        8 => Some(format!("#{}", &digits[..6])),
        _ => None,
    }
}

pub(crate) fn relative_luminance(hex: &str) -> Option<f64> {
    let channel = |range: std::ops::Range<usize>| {
        let value = f64::from(u8::from_str_radix(hex.get(range)?, 16).ok()?) / 255.0;
        Some(if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        })
    };
    Some(0.2126 * channel(1..3)? + 0.7152 * channel(3..5)? + 0.0722 * channel(5..7)?)
}

fn revision(bytes: &[u8]) -> String {
    let mut hash = Fnv::new();
    hash.write(bytes);
    format!("{:016x}", hash.finish())
}

/// FNV-1a: a change detector for file bytes, never a security boundary.
struct Fnv(u64);

impl Fnv {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use interface_contracts::appearance::{CustomThemeName, ThemeKind};
    use tempfile::TempDir;

    use super::{
        CUSTOM_THEME_LOOK_PROPERTY, CustomThemeLibrary, custom_themes_directory, declarations,
    };

    fn name(value: &str) -> CustomThemeName {
        CustomThemeName::new(value).unwrap()
    }

    #[test]
    fn says_whether_a_theme_paints_a_backdrop_and_whether_it_can_move() {
        let temp = TempDir::new().unwrap();
        let dir = custom_themes_directory(temp.path());
        let library = CustomThemeLibrary::new(dir.clone());
        library.ensure_directory().unwrap();
        let scene = format!(
            ":root {{ --theme-kind: dark; --theme-backdrop: #000; --theme-scene: url(\"data:image/svg+xml;base64,{}\"); }}",
            "QUJD".repeat(400)
        );
        fs::write(dir.join("a scene.css"), scene).unwrap();
        fs::write(
            dir.join("b shader.css"),
            ":root { --theme-shader: url(\"data:x-shader/x-fragment;base64,AAAA\"); --theme-shader-rate: 20; }",
        )
        .unwrap();
        fs::write(
            dir.join("c still.css"),
            ":root { --theme-backdrop: linear-gradient(#000, #123); --theme-scene: none; }",
        )
        .unwrap();
        fs::write(
            dir.join("d plain.css"),
            ":root { --theme-background: #101010; --theme-backdrop: NONE; --theme-look: moving; }",
        )
        .unwrap();
        let looks: Vec<_> = library
            .list()
            .iter()
            .map(|theme| {
                theme
                    .preview
                    .get(CUSTOM_THEME_LOOK_PROPERTY)
                    .map(String::as_str)
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(
            looks,
            [
                Some("moving".to_owned()),
                Some("moving".to_owned()),
                Some("still".to_owned()),
                // A file cannot claim a look it does not paint.
                None,
            ]
        );
        // The scene itself is never copied into the swatch.
        assert!(
            library.list()[0]
                .preview
                .values()
                .all(|value| value.len() <= 128)
        );
    }

    /// The look a theme file in `dir` is listed with.
    fn look_of(dir: &std::path::Path, file: &str, css: &str) -> Option<String> {
        let library = CustomThemeLibrary::new(dir.to_path_buf());
        fs::write(dir.join(file), css).unwrap();
        let stem = file.strip_suffix(".css").unwrap();
        library
            .list()
            .into_iter()
            .find(|theme| theme.name == stem)
            .and_then(|theme| theme.preview.get(CUSTOM_THEME_LOOK_PROPERTY).cloned())
    }

    /// Review, 2026-11-15: the play mark means "this theme moves". A file
    /// whose backdrop layers animate is marked although it sets no scene or
    /// shader, and a scene with nothing in it that moves is not.
    #[test]
    fn marks_a_theme_moving_when_its_backdrop_layers_animate() {
        let temp = TempDir::new().unwrap();
        let dir = custom_themes_directory(temp.path());
        CustomThemeLibrary::new(dir.clone())
            .ensure_directory()
            .unwrap();
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");

        // docs/THEMES.md's own Layers example, under its first look.
        let guide = fs::read_to_string(docs.join("THEMES.md")).unwrap();
        let layers = guide.split("### Layers").nth(1).unwrap();
        let layers = layers
            .split("```css")
            .nth(1)
            .unwrap()
            .split("```")
            .next()
            .unwrap();
        let example = format!(
            ":root {{ --theme-kind: dark; --theme-backdrop: linear-gradient(180deg, #040714, #14345a); }}\n{layers}"
        );
        assert_eq!(
            look_of(&dir, "Layers.css", &example).as_deref(),
            Some("moving")
        );
        // A layer set running by a rule of the file's own.
        let runaway = ":root { --theme-kind: dark; --theme-backdrop: linear-gradient(180deg, #050510, #102040); }
.theme-backdrop-layer[data-layer=\"1\"] { animation: runaway-sway 4s ease-in-out infinite alternate; }
@keyframes runaway-sway { from { transform: translateX(-10%); } to { transform: translateX(10%); } }";
        assert_eq!(
            look_of(&dir, "Runaway Layer.css", runaway).as_deref(),
            Some("moving")
        );
        // Keyframes that only the sidebar uses do not move the backdrop.
        let sidebar = ":root { --theme-backdrop: #000; } .sidebar { animation: glow 2s infinite; }
@keyframes glow { to { opacity: .5; } }
.theme-backdrop-layer[data-layer=\"2\"] { animation: none; }";
        assert_eq!(
            look_of(&dir, "Sidebar.css", sidebar).as_deref(),
            Some("still")
        );
    }

    #[test]
    fn marks_a_scene_moving_only_when_something_in_it_moves() {
        let temp = TempDir::new().unwrap();
        let dir = custom_themes_directory(temp.path());
        CustomThemeLibrary::new(dir.clone())
            .ensure_directory()
            .unwrap();
        // A scene of shapes alone, base64 and percent-encoded, stands still.
        let still_svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><style>rect { fill: #123; }</style><rect width=\"10\" height=\"5\"/></svg>";
        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, still_svg);
        let scene = |value: &str| {
            format!(":root {{ --theme-kind: dark; --theme-scene: url(\"{value}\"); }}")
        };
        assert_eq!(
            look_of(
                &dir,
                "Still Scene.css",
                &scene(&format!("data:image/svg+xml;base64,{base64}"))
            )
            .as_deref(),
            Some("still")
        );
        let encoded: String = still_svg
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect();
        assert_eq!(
            look_of(
                &dir,
                "Still Encoded.css",
                &scene(&format!("data:image/svg+xml,{encoded}"))
            )
            .as_deref(),
            Some("still")
        );
        // The same scene with one SMIL animation, or one CSS animation, moves.
        let smil = still_svg.replace("/></svg>", "><animate attributeName=\"y\" values=\"0;5;0\" dur=\"4s\" repeatCount=\"indefinite\"/></rect></svg>");
        let smil = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, smil);
        assert_eq!(
            look_of(
                &dir,
                "Smil Scene.css",
                &scene(&format!("data:image/svg+xml;base64,{smil}"))
            )
            .as_deref(),
            Some("moving")
        );
        let css = still_svg.replace("fill: #123;", "fill: #123; animation: drift 9s infinite;");
        let css = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, css);
        assert_eq!(
            look_of(
                &dir,
                "Css Scene.css",
                &scene(&format!("data:image/svg+xml;base64,{css}"))
            )
            .as_deref(),
            Some("moving")
        );
    }

    #[test]
    fn the_shipped_examples_keep_their_play_marks() {
        let temp = TempDir::new().unwrap();
        let dir = custom_themes_directory(temp.path());
        CustomThemeLibrary::new(dir.clone())
            .ensure_directory()
            .unwrap();
        let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        // Six move, fifteen are palettes.
        let examples = docs.join("theme-examples");
        let mut answers = Vec::new();
        for folder in [examples.clone(), examples.join("tn-code")] {
            for entry in fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("css") {
                    continue;
                }
                let file = path.file_name().unwrap().to_str().unwrap().to_owned();
                let look = look_of(&dir, &file, &fs::read_to_string(&path).unwrap());
                answers.push((file.trim_end_matches(".css").to_owned(), look));
            }
        }
        answers.sort();
        let moving: Vec<_> = answers
            .iter()
            .filter(|(_, look)| look.as_deref() == Some("moving"))
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            moving,
            [
                "Aurora",
                "Glyph",
                "Neon Grid",
                "Orbit",
                "Paper Tide",
                "TN Code"
            ]
        );
        assert_eq!(
            answers.iter().filter(|(_, look)| look.is_none()).count(),
            15
        );
        assert_eq!(answers.len(), 21);
    }

    #[test]
    fn lists_visible_css_files_by_name_and_reads_their_facts() {
        let temp = TempDir::new().unwrap();
        let dir = custom_themes_directory(temp.path());
        let library = CustomThemeLibrary::new(dir.clone());
        assert!(library.list().is_empty());
        assert_eq!(library.fingerprint(), 0);
        library.ensure_directory().unwrap();
        fs::write(
            dir.join("moss.css"),
            "/* Moss: --theme-kind: light; is only a comment */\n:root {\n  --theme-kind: dark;\n  --theme-background: #0F1310;\n  --theme-accent: oklch(80% 0.1 150);\n  --status-working: #f2a65a !important;\n}\n.sidebar { --theme-accent: red; }\n",
        )
        .unwrap();
        fs::write(dir.join("Paper Lantern.css"), ":root{--editor-bg:#f3efe6}").unwrap();
        fs::write(dir.join(".hidden.css"), ":root{}").unwrap();
        fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        fs::create_dir(dir.join("folder.css")).unwrap();
        let listed = library.list();
        let names: Vec<_> = listed.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["moss", "Paper Lantern"]);
        let moss = &listed[0];
        assert_eq!(moss.id.to_string(), "custom:moss");
        assert_eq!(moss.kind, ThemeKind::Dark);
        assert_eq!(moss.background.as_deref(), Some("#0f1310"));
        assert_eq!(moss.preview["--theme-accent"], "oklch(80% 0.1 150)");
        assert_eq!(moss.preview["--status-working"], "#f2a65a");
        assert!(moss.problem.is_none());
        let paper = &listed[1];
        assert_eq!(paper.kind, ThemeKind::Light, "inferred from its background");
        assert!(
            library
                .read(&name("moss"))
                .unwrap()
                .css
                .unwrap()
                .contains("--theme-kind: dark")
        );
        assert!(library.read(&name("absent")).is_none());
        assert!(library.read(&name("folder")).is_none());
        assert_ne!(library.fingerprint(), 0);
    }

    #[test]
    fn saving_a_file_changes_its_revision_and_the_fingerprint() {
        let temp = TempDir::new().unwrap();
        let library = CustomThemeLibrary::new(temp.path().to_path_buf());
        let path = temp.path().join("Moss.css");
        fs::write(&path, ":root{--accent:#111111}").unwrap();
        let before = (library.read(&name("Moss")).unwrap(), library.fingerprint());
        fs::write(&path, ":root{--accent:#222222;}").unwrap();
        let after = (library.read(&name("Moss")).unwrap(), library.fingerprint());
        assert_ne!(before.0.summary.revision, after.0.summary.revision);
        assert_ne!(before.1, after.1);
        assert!(after.0.css.unwrap().contains("#222222"));
        fs::remove_file(&path).unwrap();
        assert!(library.read(&name("Moss")).is_none());
        assert!(!library.is_paintable(&name("Moss")));
    }

    #[test]
    fn oversized_and_binary_files_are_listed_with_a_problem() {
        let temp = TempDir::new().unwrap();
        let library = CustomThemeLibrary::new(temp.path().to_path_buf());
        fs::write(temp.path().join("Big.css"), vec![b' '; 1024 * 1024 + 1]).unwrap();
        fs::write(temp.path().join("Binary.css"), [0xff, 0xfe, 0x00]).unwrap();
        let listed = library.list();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|theme| theme.problem.is_some()));
        assert!(!library.is_paintable(&name("Big")));
        assert!(!library.is_paintable(&name("Binary")));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_and_linked_files_are_followed_for_reading() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let dotfiles = temp.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        fs::write(dotfiles.join("Moss.css"), ":root{--theme-kind:light}").unwrap();
        let linked = temp.path().join("themes");
        symlink(&dotfiles, &linked).unwrap();
        symlink(dotfiles.join("Moss.css"), dotfiles.join("Alias.css")).unwrap();
        let library = CustomThemeLibrary::new(linked);
        library.ensure_directory().unwrap();
        let listed = library.list();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|theme| theme.kind == ThemeKind::Light));
    }

    #[test]
    fn declarations_ignore_comments_and_keep_nested_parentheses() {
        let found = declarations(
            "/* --a: no; */ :root { --a: color-mix(in srgb, red 20%, blue); --b:#fff } .x{--a: later}",
        );
        assert_eq!(found["--a"], "color-mix(in srgb, red 20%, blue)");
        assert_eq!(found["--b"], "#fff");
        assert_eq!(declarations(":root{--open: #abc")["--open"], "#abc");
        assert!(declarations("--empty: /* never closed").is_empty());
    }
}
