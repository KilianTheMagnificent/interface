use super::*;
use crate::CustomThemeLibrary;
use interface_contracts::appearance::CustomThemeName;

fn plist_colour(key: &str, red: f64, green: f64, blue: f64) -> String {
    format!(
        "\t<key>{key}</key>\n\t<dict>\n\t\t<key>Alpha Component</key>\n\t\t<real>1</real>\n\
         \t\t<key>Blue Component</key>\n\t\t<real>{blue}</real>\n\
         \t\t<key>Color Space</key>\n\t\t<string>sRGB</string>\n\
         \t\t<key>Green Component</key>\n\t\t<real>{green}</real>\n\
         \t\t<key>Red Component</key>\n\t\t<real>{red}</real>\n\t</dict>\n"
    )
}

fn iterm2_fixture() -> String {
    let mut body = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    // Dracula: background #282a36, foreground #f8f8f2, blue #bd93f9.
    body.push_str(&plist_colour(
        "Ansi 0 Color",
        0.129_411_77,
        0.133_333_34,
        0.172_549_02,
    ));
    body.push_str(&plist_colour(
        "Ansi 4 Color",
        0.741_176_5,
        0.576_470_6,
        0.976_470_6,
    ));
    body.push_str(&plist_colour("Ansi 15 Color", 1.0, 1.0, 1.0));
    body.push_str(&plist_colour(
        "Background Color",
        0.156_862_75,
        0.164_705_89,
        0.211_764_71,
    ));
    body.push_str(&plist_colour(
        "Foreground Color",
        0.972_549_03,
        0.972_549_03,
        0.949_019_6,
    ));
    body.push_str(&plist_colour(
        "Cursor Color",
        0.972_549_03,
        0.972_549_03,
        0.949_019_6,
    ));
    body.push_str("</dict>\n</plist>\n");
    body
}

fn convert(text: &str) -> ImportedScheme {
    convert_scheme(text.as_bytes(), None, "Fixture").expect("converts")
}

#[test]
fn iterm2_plist_colours_become_theme_keys_and_ansi() {
    let scheme = convert(&iterm2_fixture());
    assert_eq!(scheme.format, SchemeFormat::Iterm2);
    assert_eq!(scheme.kind, ThemeKind::Dark);
    let css = &scheme.css;
    assert!(css.contains("--theme-kind: dark;"), "{css}");
    assert!(css.contains("--theme-background: #282a36;"), "{css}");
    assert!(css.contains("--theme-text: #f8f8f2;"), "{css}");
    assert!(css.contains("--theme-screen: #282a36;"), "{css}");
    assert!(css.contains("--theme-accent: #bd93f9;"), "{css}");
    assert!(css.contains("--ansi-black: #21222c;"), "{css}");
    assert!(css.contains("--ansi-bright-white: #ffffff;"), "{css}");
    // Keys the file leaves out are left to the app's defaults.
    assert!(!css.contains("--ansi-red"), "{css}");
    assert!(!css.contains("Cursor"), "{css}");
}

#[test]
fn ghostty_theme_with_palette_lines() {
    let scheme = convert(
        "# Ghostty theme\npalette = 0=#073642\npalette = 1=#dc322f\npalette = 12=268bd2\n\
         background = fdf6e3\nforeground = #657b83\ncursor-color = #586e75\n",
    );
    assert_eq!(scheme.format, SchemeFormat::Ghostty);
    assert_eq!(scheme.kind, ThemeKind::Light);
    let css = &scheme.css;
    assert!(css.contains("--theme-kind: light;"), "{css}");
    assert!(css.contains("--theme-background: #fdf6e3;"), "{css}");
    assert!(css.contains("--ansi-red: #dc322f;"), "{css}");
    assert!(css.contains("--ansi-bright-blue: #268bd2;"), "{css}");
    // No ANSI blue, so the accent falls back to bright blue.
    assert!(css.contains("--theme-accent: #268bd2;"), "{css}");
}

#[test]
fn alacritty_toml_tables_dotted_keys_and_inline_tables() {
    let scheme = convert(
        "# Alacritty\n[colors.primary]\nbackground = '#1d1f21' # canvas\nforeground = \"0xc5c8c6\"\n\n\
         [colors.normal]\nblack = '#282a2e'\nblue = '#5f819d'\n\n\
         [colors]\nbright = { red = '#cc6666', white = '#ffffff' }\n",
    );
    assert_eq!(scheme.format, SchemeFormat::Alacritty);
    let css = &scheme.css;
    assert!(css.contains("--theme-background: #1d1f21;"), "{css}");
    assert!(css.contains("--theme-text: #c5c8c6;"), "{css}");
    assert!(css.contains("--ansi-black: #282a2e;"), "{css}");
    assert!(css.contains("--ansi-blue: #5f819d;"), "{css}");
    assert!(css.contains("--ansi-bright-red: #cc6666;"), "{css}");
    assert!(css.contains("--ansi-bright-white: #ffffff;"), "{css}");
}

#[test]
fn base16_flat_and_palette_schemes_use_the_shell_mapping() {
    let flat = convert(
        "scheme: \"Tomorrow Night\"\nauthor: \"Chris Kempson\"\nbase00: \"1d1f21\"\nbase01: \"282a2e\"\n\
         base03: \"969896\"\nbase05: \"c5c8c6\"\nbase07: \"ffffff\"\nbase08: \"cc6666\"\nbase0D: \"81a2be\"\n",
    );
    assert_eq!(flat.format, SchemeFormat::Base16);
    assert_eq!(flat.name.as_deref(), Some("Tomorrow Night"));
    let css = &flat.css;
    assert!(css.starts_with("/* Tomorrow Night\n"), "{css}");
    assert!(css.contains("(Fixture)"), "{css}");
    assert!(css.contains("--theme-raised: #282a2e;"), "{css}");
    assert!(css.contains("--theme-accent: #81a2be;"), "{css}");
    assert!(css.contains("--ansi-red: #cc6666;"), "{css}");
    assert!(css.contains("--ansi-bright-red: #cc6666;"), "{css}");
    assert!(css.contains("--ansi-bright-black: #969896;"), "{css}");
    assert!(css.contains("--ansi-white: #c5c8c6;"), "{css}");
    assert!(css.contains("--ansi-bright-white: #ffffff;"), "{css}");

    let nested = convert(
        "system: \"base16\"\nname: \"Paper\"\nvariant: \"light\"\npalette:\n  base00: \"#f2eede\"\n  base05: \"#000000\"\n",
    );
    assert_eq!(nested.kind, ThemeKind::Light);
    assert!(
        nested.css.contains("--theme-background: #f2eede;"),
        "{}",
        nested.css
    );
}

#[test]
fn vscode_jsonc_with_comments_trailing_commas_and_alpha() {
    let scheme = convert(
        r##"// A VS Code theme
{
  "$schema": "vscode://schemas/color-theme",
  "name": "Night Owl",
  "type": "dark",
  /* editor */
  "colors": {
    "editor.background": "#011627",
    "editor.foreground": "#d6deeb",
    "sideBar.background": "#011627",
    "button.background": "#7e57c2cc",
    "terminal.ansiRed": "#EF5350", // red
    "terminal.ansiBrightCyan": "#7fdbca",
  },
  "tokenColors": [],
}
"##,
    );
    assert_eq!(scheme.format, SchemeFormat::VsCode);
    assert_eq!(scheme.name.as_deref(), Some("Night Owl"));
    let css = &scheme.css;
    assert!(css.contains("--theme-background: #011627;"), "{css}");
    assert!(css.contains("--theme-panel: #011627;"), "{css}");
    assert!(css.contains("--theme-accent: #7e57c2;"), "{css}");
    assert!(css.contains("--ansi-red: #ef5350;"), "{css}");
    assert!(css.contains("--ansi-bright-cyan: #7fdbca;"), "{css}");
}

#[test]
fn vscode_terminal_colours_paint_the_screen_separately() {
    let scheme = convert(
        r##"{"type": "light", "colors": {"editor.background": "#ffffff", "terminal.background": "#f3f3f3", "terminal.foreground": "#333333"}}"##,
    );
    let css = &scheme.css;
    assert!(css.contains("--theme-kind: light;"), "{css}");
    assert!(css.contains("--theme-background: #ffffff;"), "{css}");
    assert!(css.contains("--theme-screen: #f3f3f3;"), "{css}");
    assert!(css.contains("--theme-screen-text: #333333;"), "{css}");
}

#[test]
fn format_override_beats_detection() {
    // Looks like Ghostty to detection, but the caller says Alacritty.
    let error = convert_scheme(
        b"background = #000000\n",
        Some(SchemeFormat::Alacritty),
        "x",
    )
    .unwrap_err();
    assert_eq!(error, SchemeImportError::NoColours(SchemeFormat::Alacritty));
}

#[test]
fn refuses_what_it_cannot_convert() {
    assert_eq!(
        convert_scheme(b"hello world\n", None, "x").unwrap_err(),
        SchemeImportError::UnknownFormat
    );
    assert_eq!(
        convert_scheme(b"bplist00\x01\x02", None, "x").unwrap_err(),
        SchemeImportError::BinaryPlist
    );
    assert_eq!(
        convert_scheme(&[0xff, 0xfe, 0x00], None, "x").unwrap_err(),
        SchemeImportError::NotText
    );
    let large = vec![b' '; usize::try_from(SCHEME_IMPORT_MAX_BYTES).unwrap() + 1];
    assert_eq!(
        convert_scheme(&large, None, "x").unwrap_err(),
        SchemeImportError::TooLarge
    );
    assert!(matches!(
        convert_scheme(b"{ not json", Some(SchemeFormat::VsCode), "x").unwrap_err(),
        SchemeImportError::InvalidJson(SchemeFormat::VsCode, _)
    ));
    assert_eq!(
        convert_scheme(b"{\"colors\": {}}", None, "x").unwrap_err(),
        SchemeImportError::NoColours(SchemeFormat::VsCode)
    );
}

#[test]
fn names_cannot_break_out_of_the_header_comment() {
    let scheme = convert_scheme(
        b"{\"name\": \"Evil */ :root { --x: 1 } /*\\u001b[31m\", \"colors\": {\"editor.background\": \"#000000\"}}",
        None,
        "evil",
    )
    .unwrap();
    let header = scheme.css.split("\n:root {\n").next().unwrap();
    assert_eq!(header.matches("*/").count(), 1, "{header}");
    assert!(!scheme.css.contains('\u{1b}'));
}

#[test]
fn hex_spellings() {
    assert_eq!(Rgb::parse("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
    assert_eq!(Rgb::parse("'0xC5C8C6'"), Some(Rgb(0xc5, 0xc8, 0xc6)));
    assert_eq!(Rgb::parse("112233ff"), Some(Rgb(0x11, 0x22, 0x33)));
    assert_eq!(Rgb::parse("red"), None);
    assert_eq!(Rgb::parse("#12345"), None);
}

#[test]
fn jsonc_stripping_keeps_strings() {
    let plain = strip_jsonc("{\"a\": \"x // y /* z */\", // c\n \"b\": [1, 2,],}");
    let value: serde_json::Value = serde_json::from_str(&plain).unwrap();
    assert_eq!(value["a"], "x // y /* z */");
    assert_eq!(value["b"], serde_json::json!([1, 2]));
}

/// The CSS a conversion writes reads back through the theme library as the
/// same kind and window fill, with a swatch.
#[test]
fn converted_css_reads_back_as_a_theme_file() {
    let directory = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("Dracula", iterm2_fixture()),
        (
            "Solarized",
            "background = fdf6e3\nforeground = 657b83\n".to_owned(),
        ),
    ] {
        let scheme = convert(&text);
        std::fs::write(directory.path().join(format!("{name}.css")), &scheme.css).unwrap();
        let library = CustomThemeLibrary::new(directory.path().to_owned());
        let file = library
            .read(&CustomThemeName::new(name).unwrap())
            .expect("listed");
        assert_eq!(file.summary.problem, None);
        assert_eq!(file.summary.kind, scheme.kind);
        assert!(file.summary.background.is_some());
        assert!(file.summary.preview.contains_key("--theme-background"));
        assert!(file.summary.preview.contains_key("--theme-accent") || name == "Solarized");
    }
}
