//! `iface theme convert`: another app's colour scheme as an Interface theme
//! file. The conversion lives in `interface_settings::theme_import`; this
//! adapter reads the source within bounds and prints the CSS. It never writes
//! into the themes folder: the user redirects the output there.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use interface_contracts::appearance::{CustomThemeName, ThemeKind};
use interface_settings::{SCHEME_IMPORT_MAX_BYTES, SchemeFormat, convert_scheme};

use crate::errors::{CLI_EXIT, CommandResult};
use crate::sanitize::for_display;

pub const THEME_USAGE: &str = "Usage: iface theme convert <file> [--format <format>]

Prints an Interface theme file converted from another app's colour scheme.
Save it in ~/.config/interface/themes and pick it in Settings > Appearance:

  iface theme convert Dracula.itermcolors > ~/.config/interface/themes/Dracula.css

Formats (detected from the file unless --format is given):
  iterm2     iTerm2 .itermcolors (XML property list)
  ghostty    Ghostty theme (palette = N=#rrggbb)
  alacritty  Alacritty TOML colours ([colors.primary], [colors.normal], ...)
  base16     Base16 or Base24 YAML scheme (base00 ... base0F)
  vscode     VS Code colour theme JSON (comments allowed)";

#[must_use]
pub fn is_help_request(args: &[String]) -> bool {
    args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h")
}

/// Runs a `theme` verb. `cwd` resolves a relative source path.
#[must_use]
pub fn run(args: &[String], cwd: &Path) -> CommandResult {
    if is_help_request(args) {
        return CommandResult::success(format!("{THEME_USAGE}\n"));
    }
    let (source, format) = match options(args) {
        Ok(options) => options,
        Err(message) => {
            return CommandResult::error(
                format!("{message}\n\n{THEME_USAGE}"),
                CLI_EXIT.validation,
            );
        }
    };
    let path = cwd.join(source);
    let bytes = match read_source(&path) {
        Ok(bytes) => bytes,
        Err(message) => return CommandResult::error(message, CLI_EXIT.generic),
    };
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let scheme = match convert_scheme(&bytes, format, &stem) {
        Ok(scheme) => scheme,
        Err(error) => {
            return CommandResult::error(format!("{}: {error}", path.display()), CLI_EXIT.generic);
        }
    };
    let kind = match scheme.kind {
        ThemeKind::Light => "light",
        ThemeKind::Dark => "dark",
    };
    let file_name = [scheme.name.as_deref(), Some(stem.as_str())]
        .into_iter()
        .flatten()
        .find_map(|name| CustomThemeName::new(name).ok())
        .map_or_else(|| "<Name>.css".to_owned(), |name| name.file_name());
    CommandResult {
        stdout: scheme.css,
        stderr: format!(
            "iface: converted a {kind} {} scheme. Save it as ~/.config/interface/themes/{}\n",
            scheme.format,
            for_display(&file_name)
        ),
        exit_code: CLI_EXIT.success,
    }
}

fn options(args: &[String]) -> Result<(&str, Option<SchemeFormat>), String> {
    let mut rest = args.iter().map(String::as_str);
    match rest.next() {
        Some("convert") => {}
        Some(verb) => return Err(format!("Unknown theme command: {verb}")),
        None => return Err("theme requires a command".into()),
    }
    let mut source = None;
    let mut format = None;
    while let Some(arg) = rest.next() {
        match arg {
            "--format" if format.is_none() => {
                let value = rest.next().ok_or("--format requires a value")?;
                format = Some(SchemeFormat::parse(value).ok_or_else(|| {
                    format!(
                        "--format must be one of: {}",
                        SchemeFormat::ALL.map(SchemeFormat::as_str).join(", ")
                    )
                })?);
            }
            "--format" => return Err("--format may only be specified once".into()),
            flag if flag.starts_with("--") => return Err(format!("Unknown option: {flag}")),
            path if source.is_none() => source = Some(path),
            extra => return Err(format!("Unexpected argument: {extra}")),
        }
    }
    let source = source.ok_or("theme convert requires a scheme file")?;
    Ok((source, format))
}

fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    let display = path.display();
    let file = File::open(path).map_err(|error| format!("{display}: {error}"))?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(format!("{display}: not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(SCHEME_IMPORT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{display}: {error}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn theme_convert_prints_css_and_suggests_the_file_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("Solarized Light.ghostty"),
            "background = fdf6e3\nforeground = 657b83\npalette = 4=#268bd2\n",
        )
        .unwrap();
        let result = run(
            &args(&["convert", "Solarized Light.ghostty"]),
            directory.path(),
        );
        assert_eq!(result.exit_code, CLI_EXIT.success, "{}", result.stderr);
        assert!(
            result.stdout.contains("--theme-kind: light;"),
            "{}",
            result.stdout
        );
        assert!(
            result.stdout.contains("--ansi-blue: #268bd2;"),
            "{}",
            result.stdout
        );
        assert_eq!(
            result.stderr,
            "iface: converted a light Ghostty scheme. Save it as \
             ~/.config/interface/themes/Solarized Light.css\n"
        );
    }

    #[test]
    fn theme_convert_honours_format_and_reports_failures() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("scheme.txt"),
            "background = #000000\n",
        )
        .unwrap();
        let forced = run(
            &args(&["convert", "scheme.txt", "--format", "alacritty"]),
            directory.path(),
        );
        assert_eq!(forced.exit_code, CLI_EXIT.generic);
        assert!(
            forced
                .stderr
                .contains("No colours were found in the Alacritty file.")
        );
        assert!(forced.stdout.is_empty());

        let missing = run(&args(&["convert", "absent.itermcolors"]), directory.path());
        assert_eq!(missing.exit_code, CLI_EXIT.generic);
        assert!(missing.stderr.contains("absent.itermcolors"));

        for bad in [
            &["convert"][..],
            &["convert", "a", "b"],
            &["convert", "a", "--format", "kitty"],
            &["convert", "a", "--format"],
            &["install", "a"],
        ] {
            let result = run(&args(bad), directory.path());
            assert_eq!(result.exit_code, CLI_EXIT.validation, "{bad:?}");
        }
    }

    #[test]
    fn theme_help() {
        let directory = tempfile::tempdir().unwrap();
        for help in [&[][..], &["--help"], &["convert", "--help"]] {
            let result = run(&args(help), directory.path());
            assert_eq!(result.exit_code, CLI_EXIT.success);
            assert!(result.stdout.starts_with("Usage: iface theme convert"));
        }
    }
}
