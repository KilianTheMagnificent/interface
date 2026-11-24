//! The shell walk: one command's segments, wrappers, working directories and
//! the per-segment checks that send a command to the owner.

use std::path::{Path, PathBuf};

use interface_common::paths::lexically_normalized;
use interface_workspace::is_credential_path;

use crate::agent_shell::{self, Segment};

use super::paths::{
    Context, Effect, brace_groups_as_wildcards, expand_glob, expand_home, real_path,
};
use super::risk::{has_argument_checks, risky};
use super::targets::{
    basename, file_targets, git_invocation, git_only_reads, operands, read_operand_options,
    read_operands,
};
use super::{
    AgentScope, CREDENTIAL, CWD_OUTSIDE, INTERPRETER, MAX_WORKING_DIRECTORIES, PolicyDecision,
    RM_ROOT, UNPARSEABLE, UNRESOLVED, Verdict, allow, ask, known_command_effects,
};

pub(super) fn shell(command: &str, cwd: Option<&str>, scope: &AgentScope) -> PolicyDecision {
    shell_checked(command, cwd, scope, false)
}
pub(super) fn shell_checked(
    command: &str,
    cwd: Option<&str>,
    scope: &AgentScope,
    strict: bool,
) -> PolicyDecision {
    let Ok(segments) = agent_shell::parse(command) else {
        return ask(UNPARSEABLE);
    };
    if segments.iter().any(|segment| segment.substitution) {
        return ask("shellSubstitution");
    }
    let mut context = Context::new(scope);
    context.require_known_effects = strict;
    let start = cwd.map_or_else(
        || scope.cwd.clone(),
        |cwd| scope.cwd.join(expand_home(cwd, &scope.home_dir)),
    );
    let mut state = ShellState {
        working_directories: Some(vec![start]),
        created: Vec::new(),
        looping: segments.iter().any(|segment| {
            segment
                .words
                .iter()
                .any(|word| LOOP_KEYWORDS.contains(&word.as_str()))
        }),
    };
    for segment in &segments {
        if let Err(reason) = context.check_segment(segment, &mut state) {
            return ask(reason);
        }
    }
    // Claude's Bash tool keeps the working directory for its next command, whose relative
    // paths the policy resolves against scope: no `cd` may leave it outside scope or unknown.
    // The first candidate is where the command started, which the provider reports each time.
    let moved = state
        .working_directories
        .as_deref()
        .map(|directories| directories.get(1..).unwrap_or_default());
    if !context.all_in_scope(moved) {
        return ask(CWD_OUTSIDE);
    }
    allow("shellInScope")
}

struct ShellState {
    /// Every directory a relative path could be relative to at this point:
    /// `cd` adds candidates and never removes one, because `||`, pipes, failed
    /// `cd`s, and operators the tokenizer cannot pair (a list continued across a
    /// newline or `#`) leave the earlier directory in effect. `None` once unknown.
    working_directories: Option<Vec<PathBuf>>,
    /// Paths a link, move or copy earlier in the command may have replaced.
    created: Vec<PathBuf>,
    /// A loop may repeat a `cd` from wherever its last pass ended.
    looping: bool,
}

/// Builtins that change the shell's working directory; zsh's `chdir` is `cd`.
pub(super) const DIRECTORY_CHANGES: &[&str] = &["cd", "chdir", "pushd", "popd"];
/// Keywords whose body may run more than once.
const LOOP_KEYWORDS: &[&str] = &["for", "foreach", "while", "until", "repeat"];
/// Variables `cd` resolves bare names through.
const CD_SEARCH_VARIABLES: &[&str] = &["CDPATH", "cdpath"];

/// A word that rewrites how `cd` or `~` resolve for the rest of the command:
/// `HOME` assigned, appended or subscripted; `CDPATH`/`cdpath` named at all
/// (`cdpath=/`, `set -A cdpath`, `read CDPATH`, `typeset -n ref=cdpath`); or
/// zsh/bash `cdablevars`/`autocd` in any spelling (`setopt CDABLE_VARS`,
/// `shopt -s cdable_vars`, `options[autocd]=on`).
fn rewrites_directory_lookup(word: &str) -> bool {
    let assigns = |name: &str, text: &str| {
        text.strip_prefix(name)
            .is_some_and(|rest| rest.starts_with(['=', '[']) || rest.starts_with("+="))
    };
    let names_search = |text: &str| {
        CD_SEARCH_VARIABLES
            .iter()
            .any(|name| text == *name || assigns(name, text))
    };
    let option = word.replace('_', "").to_ascii_lowercase();
    assigns("HOME", word)
        || names_search(word)
        || word
            .split_once('=')
            .is_some_and(|(_, value)| names_search(value))
        || option.contains("cdablevars")
        || option.contains("autocd")
}

/// zsh's single-letter option spellings (`-T` is cdablevars, `-J` autocd), and
/// `setopt -m`, which sets every option matching a pattern.
fn sets_directory_option_letter(program: &str, args: &[String]) -> bool {
    let letters: &[char] = match program {
        "set" => &['T', 'J'],
        "setopt" | "unsetopt" => &['T', 'J', 'm'],
        _ => return false,
    };
    args.iter().any(|word| {
        word.starts_with(['-', '+']) && !word.starts_with("--") && word.contains(letters)
    })
}

/// Leading words that are not the program: control keywords and `NAME=value`.
const KEYWORDS: &[&str] = &[
    "!",
    "{",
    "}",
    "if",
    "then",
    "else",
    "elif",
    "fi",
    "do",
    "done",
    "while",
    "until",
    "noglob",
    "nocorrect",
    "-",
];
/// Compound commands whose bodies this tokenizer cannot delimit.
const UNSUPPORTED_KEYWORDS: &[&str] = &["function", "case", "esac", "select", "coproc"];

/// A command that runs the command after its own options.
struct Wrapper {
    name: &'static str,
    flags: &'static [&'static str],
    valued: &'static [&'static str],
    operands: usize,
}

const TIMEOUT_FLAGS: &[&str] = &[
    "-v",
    "-f",
    "-p",
    "--foreground",
    "--preserve-status",
    "--verbose",
];
const TIMEOUT_VALUED: &[&str] = &["-s", "-k", "--signal", "--kill-after"];
const NPX_FLAGS: &[&str] = &[
    "-y",
    "--yes",
    "--no",
    "-q",
    "--quiet",
    "--no-install",
    "--ignore-existing",
];
const NPX_VALUED: &[&str] = &["-p", "--package"];

// An option a wrapper is not known to take fails closed as `unparseable`: a
// misread option value would otherwise be taken for the program.
const WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "env",
        flags: &[
            "-",
            "-i",
            "-0",
            "-v",
            "--ignore-environment",
            "--null",
            "--debug",
        ],
        valued: &["-u", "--unset"],
        operands: 0,
    },
    Wrapper {
        name: "time",
        flags: &[
            "-p",
            "-l",
            "-h",
            "-a",
            "-v",
            "-q",
            "--portability",
            "--verbose",
            "--quiet",
        ],
        valued: &[],
        operands: 0,
    },
    Wrapper {
        name: "nice",
        flags: &[],
        valued: &["-n", "--adjustment"],
        operands: 0,
    },
    Wrapper {
        name: "nohup",
        flags: &[],
        valued: &[],
        operands: 0,
    },
    Wrapper {
        name: "command",
        flags: &["-p", "-v", "-V"],
        valued: &[],
        operands: 0,
    },
    Wrapper {
        name: "builtin",
        flags: &[],
        valued: &[],
        operands: 0,
    },
    Wrapper {
        name: "xargs",
        flags: &[
            "-0",
            "-r",
            "-t",
            "-p",
            "-x",
            "-o",
            "--null",
            "--no-run-if-empty",
            "--verbose",
            "--interactive",
            "--exit",
            "--open-tty",
        ],
        valued: &[
            "-n",
            "-I",
            "-L",
            "-P",
            "-s",
            "-E",
            "-d",
            "-J",
            "-R",
            "-S",
            "-a",
            "--max-args",
            "--replace",
            "--max-lines",
            "--max-procs",
            "--max-chars",
            "--eof",
            "--delimiter",
            "--arg-file",
        ],
        operands: 0,
    },
    Wrapper {
        name: "timeout",
        flags: TIMEOUT_FLAGS,
        valued: TIMEOUT_VALUED,
        operands: 1,
    },
    Wrapper {
        name: "gtimeout",
        flags: TIMEOUT_FLAGS,
        valued: TIMEOUT_VALUED,
        operands: 1,
    },
    Wrapper {
        name: "caffeinate",
        flags: &["-d", "-i", "-m", "-s", "-u"],
        valued: &["-t", "-w"],
        operands: 0,
    },
    Wrapper {
        name: "stdbuf",
        flags: &[],
        valued: &["-i", "-o", "-e", "--input", "--output", "--error"],
        operands: 0,
    },
    Wrapper {
        name: "npx",
        flags: NPX_FLAGS,
        valued: NPX_VALUED,
        operands: 0,
    },
    Wrapper {
        name: "pnpx",
        flags: NPX_FLAGS,
        valued: NPX_VALUED,
        operands: 0,
    },
    Wrapper {
        name: "bunx",
        flags: NPX_FLAGS,
        valued: NPX_VALUED,
        operands: 0,
    },
    Wrapper {
        name: "corepack",
        flags: &[],
        valued: &[],
        operands: 0,
    },
    Wrapper {
        name: "repeat",
        flags: &[],
        valued: &[],
        operands: 1,
    },
    Wrapper {
        name: "arch",
        flags: &["-x86_64", "-arm64", "-arm64e", "-i386", "-32", "-64", "-c"],
        valued: &["-d", "-e"],
        operands: 0,
    },
    Wrapper {
        name: "setsid",
        flags: &["-f", "-w", "-c", "--fork", "--wait", "--ctty"],
        valued: &[],
        operands: 0,
    },
    // Runs a developer-directory tool, or any program on `PATH`.
    Wrapper {
        name: "xcrun",
        flags: &[
            "-h",
            "--help",
            "--version",
            "-v",
            "--verbose",
            "-l",
            "--log",
            "-f",
            "--find",
            "-r",
            "--run",
            "-n",
            "--no-cache",
            "-k",
            "--kill-cache",
            "--show-sdk-path",
            "--show-sdk-version",
            "--show-sdk-build-version",
            "--show-sdk-platform-path",
            "--show-sdk-platform-version",
            "--show-toolchain-path",
        ],
        valued: &["--sdk", "--toolchain"],
        operands: 0,
    },
    Wrapper {
        name: "taskpolicy",
        flags: &["-b", "-B", "-a", "-s", "-x", "-X"],
        valued: &["-d", "-g", "-c", "-t", "-l", "-S", "-m", "-j", "-P", "-p"],
        operands: 0,
    },
];

/// Wrappers a strict task still treats as opaque, as it did before they were
/// unwrapped: they pick the binary themselves or set up an environment first.
const OPAQUE_WRAPPERS: &[&str] = &["xcrun", "taskpolicy"];

const UV_FLAGS: &[&str] = &[
    "-h",
    "--help",
    "-q",
    "--quiet",
    "-v",
    "--verbose",
    "--all-extras",
    "--no-dev",
    "--only-dev",
    "--no-default-groups",
    "--all-groups",
    "-m",
    "--module",
    "--no-editable",
    "--exact",
    "--no-env-file",
    "--isolated",
    "--active",
    "--no-sync",
    "--locked",
    "--frozen",
    "-s",
    "--script",
    "--gui-script",
    "--all-packages",
    "--no-project",
    "--no-index",
    "-U",
    "--upgrade",
    "--no-sources",
    "--reinstall",
    "--compile-bytecode",
    "--no-build-isolation",
    "--no-build",
    "--no-binary",
    "-n",
    "--no-cache",
    "--refresh",
    "--managed-python",
    "--no-managed-python",
    "--no-python-downloads",
    "--system-certs",
    "--offline",
    "--no-progress",
    "--no-config",
];
const UV_VALUED: &[&str] = &[
    "--extra",
    "--no-extra",
    "--group",
    "--no-group",
    "--only-group",
    "--no-editable-package",
    "--env-file",
    "-w",
    "--with",
    "--with-editable",
    "--with-requirements",
    "--package",
    "--python-platform",
    "--index",
    "--default-index",
    "-i",
    "--index-url",
    "--extra-index-url",
    "-f",
    "--find-links",
    "--index-strategy",
    "--keyring-provider",
    "-P",
    "--upgrade-package",
    "--upgrade-group",
    "--resolution",
    "--prerelease",
    "--prerelease-package",
    "--fork-strategy",
    "--exclude-newer",
    "--exclude-newer-package",
    "--no-sources-package",
    "--reinstall-package",
    "--link-mode",
    "-C",
    "--config-setting",
    "--config-settings-package",
    "--no-build-isolation-package",
    "--no-build-package",
    "--no-binary-package",
    "--cache-dir",
    "--refresh-package",
    "-p",
    "--python",
    "--color",
    "--allow-insecure-host",
    "--directory",
    "--project",
    "--config-file",
];

/// Python project tools whose `run` runs the named command in the project's
/// environment. Each table holds the options the tool takes before or after
/// `run`; any other option fails closed as `unparseable`.
const PROJECT_RUNNERS: &[Wrapper] = &[
    Wrapper {
        name: "uv",
        flags: UV_FLAGS,
        valued: UV_VALUED,
        operands: 0,
    },
    Wrapper {
        name: "poetry",
        flags: &[
            "-h",
            "--help",
            "-q",
            "--quiet",
            "-v",
            "-vv",
            "-vvv",
            "--verbose",
            "--ansi",
            "--no-ansi",
            "-n",
            "--no-interaction",
            "--no-plugins",
            "--no-cache",
        ],
        valued: &["-P", "--project", "-C", "--directory"],
        operands: 0,
    },
    Wrapper {
        name: "pdm",
        flags: &[
            "-h",
            "--help",
            "-q",
            "--quiet",
            "-v",
            "--verbose",
            "-g",
            "--global",
            "-l",
            "--list",
            "-j",
            "--json",
            "-s",
            "--site-packages",
            "--recreate",
        ],
        valued: &["-p", "--project", "--skip", "--venv"],
        operands: 0,
    },
    Wrapper {
        name: "hatch",
        flags: &[
            "-h",
            "--help",
            "-q",
            "--quiet",
            "-v",
            "--verbose",
            "--color",
            "--no-color",
            "--interactive",
            "--no-interactive",
        ],
        valued: &[
            "-e",
            "--env",
            "-p",
            "--project",
            "--data-dir",
            "--cache-dir",
            "--config",
        ],
        operands: 0,
    },
    Wrapper {
        name: "pipenv",
        flags: &[
            "-h",
            "--help",
            "-q",
            "--quiet",
            "-v",
            "--verbose",
            "--clear",
            "--system",
            "--bare",
            "--site-packages",
            "--no-site-packages",
        ],
        valued: &["--python", "--pypi-mirror"],
        operands: 0,
    },
];

impl Wrapper {
    /// Index of the first word after this wrapper's options and operands.
    fn skip(&self, words: &[String], mut index: usize) -> Result<usize, &'static str> {
        let mut operands = self.operands;
        let mut options = true;
        while let Some(word) = words.get(index) {
            let word = word.as_str();
            // An expansion could turn a skipped word into the program.
            let value = words.get(index + 1).map_or("", String::as_str);
            if is_dynamic_word(word) || (self.valued.contains(&word) && is_dynamic_word(value)) {
                return Err(UNRESOLVED);
            }
            if options && word == "--" {
                options = false;
                index += 1;
            } else if options
                && (self.flags.contains(&word) || (word.starts_with('-') && word != "-"))
            {
                if self.flags.contains(&word) {
                    index += 1;
                } else if self.valued.contains(&word) {
                    index += 2;
                } else if self
                    .valued
                    .iter()
                    .any(|flag| has_attached_value(word, flag))
                {
                    index += 1;
                } else {
                    return Err(UNPARSEABLE);
                }
            } else if operands > 0 {
                operands -= 1;
                index += 1;
            } else {
                break;
            }
        }
        Ok(index)
    }
}

pub(super) fn has_attached_value(word: &str, flag: &str) -> bool {
    word.strip_prefix(flag).is_some_and(|rest| {
        if flag.starts_with("--") {
            rest.starts_with('=')
        } else {
            !rest.is_empty()
        }
    })
}

/// Environment variables through which a later program runs a command, loads
/// code or reads replacement configuration. Mirrors `git -c` handling.
const EXEC_ENVIRONMENT: &[&str] = &[
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR",
    "GIT_ASKPASS",
    "GIT_PROXY_COMMAND",
    "GIT_EXEC_PATH",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "PAGER",
    "MANPAGER",
    "LESSOPEN",
    "LESSCLOSE",
    "EDITOR",
    "VISUAL",
    "SSH_ASKPASS",
    "SUDO_ASKPASS",
    "BASH_ENV",
    "ENV",
    "ZDOTDIR",
    "PROMPT_COMMAND",
    "PS4",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "NODE_OPTIONS",
    "PYTHONSTARTUP",
    "PERL5OPT",
    "RUBYOPT",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC",
];
const EXEC_ENVIRONMENT_PREFIXES: &[&str] = &["DYLD_", "GIT_CONFIG_KEY_", "GIT_CONFIG_VALUE_"];

/// Builtins whose `NAME=value` arguments set the environment of later commands.
const ASSIGNMENT_BUILTINS: &[&str] = &["export", "declare", "typeset", "readonly", "local"];

fn is_exec_environment(word: &str) -> bool {
    if !is_assignment(word) {
        return false;
    }
    let name = word.split_once('=').map_or(word, |(name, _)| name);
    let name = name.strip_suffix('+').unwrap_or(name);
    EXEC_ENVIRONMENT.contains(&name)
        || EXEC_ENVIRONMENT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        // npm reads `npm_config_*` case-insensitively.
        || name.to_ascii_lowercase().starts_with("npm_config_")
        || (name.starts_with("CARGO_TARGET_") && name.ends_with("_RUNNER"))
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A variable, brace expansion or zsh `=command` expansion.
fn is_dynamic_word(word: &str) -> bool {
    word.contains('$') || word.starts_with('=') || brace_groups(word).next().is_some()
}

/// Byte ranges of `{…}` groups the shell would expand: a `,` or `..` inside
/// and no whitespace (unquoted whitespace would have split the word).
pub(super) fn brace_groups(word: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    word.match_indices('{').filter_map(|(open, _)| {
        let close = open + word[open..].find('}')?;
        let inner = &word[open + 1..close];
        let expands =
            (inner.contains(',') || inner.contains("..")) && !inner.contains(char::is_whitespace);
        expands.then_some((open, close))
    })
}

/// `.`, `..`, or a path starting with `/`, `./`, `../` or `~`: never looked up
/// through `CDPATH` or `cdablevars`.
fn is_anchored_directory(word: &str) -> bool {
    matches!(word, "." | "..")
        || ["/", "./", "../", "~"]
            .iter()
            .any(|prefix| word.starts_with(prefix))
}

/// A word whose expansion names something the policy cannot see.
fn is_unresolved_word(word: &str) -> bool {
    word.is_empty()
        || word.contains(['$', '{', '}', '`'])
        || word.starts_with('=')
        || (word.starts_with('~') && word != "~" && !word.starts_with("~/"))
}

pub(super) fn has_glob(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// Programs whose path operands are write, move or permission targets.
pub(super) const FILE_COMMANDS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "touch", "mkdir", "tee", "truncate", "mv", "cp", "ln",
    "install", "rsync", "ditto", "chmod", "chown", "chgrp", "sed", "gsed",
];

/// Programs whose targets may be links or replaced trees afterwards.
const REPLACING_COMMANDS: &[&str] = &["ln", "mv", "cp", "install", "rsync", "ditto"];

const HARMLESS_REDIRECTS: &[&str] = &["/dev/null", "/dev/stdout", "/dev/stderr"];

impl Context<'_> {
    #[allow(clippy::too_many_lines)] // One segment's checks, in the order their reasons win.
    fn check_segment(&self, segment: &Segment, state: &mut ShellState) -> Verdict {
        let words = &segment.words;
        let Wrapping {
            start,
            via_xargs,
            via_runner,
            runner,
            program,
        } = command_start(words)?;
        // zsh brace bodies (`while true { … }`) put a second command mid-segment.
        if words
            .iter()
            .skip(start + 1)
            .any(|word| word == "{" || word == "}")
        {
            return Err(UNPARSEABLE);
        }
        // These rewrite how `cd` and `~` resolve for the rest of the command.
        if words.iter().any(|word| rewrites_directory_lookup(word)) {
            return Err(UNRESOLVED);
        }
        // Assignments before the program (including through `env`) and
        // arguments of `export`-like builtins reach later programs' environment.
        let exports = words.get(start).is_some_and(|program| {
            ASSIGNMENT_BUILTINS.contains(&basename(program).to_ascii_lowercase().as_str())
        });
        let exported = if exports { &words[start + 1..] } else { &[] };
        if words
            .iter()
            .take(start)
            .chain(exported)
            .any(|word| is_exec_environment(word))
        {
            return Err(INTERPRETER);
        }
        self.check_runner(
            runner.map(|runner| &words[runner..]),
            via_runner,
            via_xargs,
            state,
        )?;
        let mut targets: Vec<(String, Effect)> = Vec::new();
        let mut resolved = None;
        if let Some(program_word) = program {
            if is_unresolved_word(program_word)
                || (has_glob(program_word) && program_word != "[" && program_word != "[[")
            {
                return Err(UNRESOLVED);
            }
            // Fold case, strip an npx `@version`/scope, resolve a `/` path
            // through symlinks, and expand a `git-<verb>` plumbing name.
            let name = self.program_name(program_word, via_runner, state)?;
            let mut args: Vec<String> = words[start + 1..].to_vec();
            let program = match name.strip_prefix("git-").filter(|verb| !verb.is_empty()) {
                Some(verb) => {
                    args.insert(0, verb.to_string());
                    "git".to_string()
                }
                None => name,
            };
            if self.require_known_effects && (via_runner || !known_command_effects(&program)) {
                return Err("opaqueCommandNeedsOwner");
            }
            if let Some(reason) = risky(&program, &args) {
                return Err(reason);
            }
            if sets_directory_option_letter(&program, &args) {
                return Err(UNRESOLVED);
            }
            // A scoped package may be the tool its scope names.
            if via_runner {
                if let Some(reason) = package_scope(program_word)
                    .filter(|scope| *scope != program)
                    .and_then(|scope| risky(&scope, &args))
                {
                    return Err(reason);
                }
            }
            // An expansion could rewrite the subcommand or flag the check read.
            if has_argument_checks(&program) && args.iter().any(|word| is_dynamic_word(word)) {
                return Err(UNRESOLVED);
            }
            // `xargs` supplies argv from stdin that the policy never sees.
            if via_xargs {
                if FILE_COMMANDS.contains(&program.as_str())
                    || read_operand_options(&program).is_some()
                {
                    return Err(UNRESOLVED);
                }
                if has_argument_checks(&program) {
                    return Err("risky:xargs");
                }
            }
            targets = file_targets(&program, &args)
                .into_iter()
                .map(|(word, effect)| (word.to_string(), effect))
                .collect();
            self.check_targets(&program, &args, &targets, state)?;
            for word in read_operands(&program, &args, &targets) {
                self.check_shell_read(word, state)?;
            }
            resolved = Some((program, args));
        }
        self.check_redirects(segment, state)?;
        for word in words.iter().chain(&segment.inputs) {
            self.check_credential_word(word, state)?;
        }
        if let Some((program, args)) = resolved {
            if REPLACING_COMMANDS.contains(&program.as_str()) {
                for (word, _) in &targets {
                    self.record_created(word, state);
                }
            }
            self.change_directory(&program, &args, state);
        }
        Ok(())
    }

    /// A runner `command_start` unwrapped was the program before it was
    /// unwrapped, and every verdict it got then still applies.
    fn check_runner(
        &self,
        runner: Option<&[String]>,
        via_runner: bool,
        via_xargs: bool,
        state: &ShellState,
    ) -> Verdict {
        let Some([word, args @ ..]) = runner else {
            return Ok(());
        };
        if is_unresolved_word(word) || has_glob(word) {
            return Err(UNRESOLVED);
        }
        let name = self.program_name(word, via_runner, state)?;
        if self.require_known_effects {
            return Err("opaqueCommandNeedsOwner");
        }
        if let Some(reason) = risky(&name, args) {
            return Err(reason);
        }
        if has_argument_checks(&name) {
            if args.iter().any(|word| is_dynamic_word(word)) {
                return Err(UNRESOLVED);
            }
            if via_xargs {
                return Err("risky:xargs");
            }
        }
        Ok(())
    }

    /// The program's matchable name: lowercased basename, with a runner's
    /// `@version`/scope stripped, and a `/` path resolved through symlinks to
    /// its real basename. A program created or replaced earlier in the same
    /// command has an unknowable identity.
    fn program_name(
        &self,
        word: &str,
        via_runner: bool,
        state: &ShellState,
    ) -> Result<String, &'static str> {
        let token = if via_runner {
            strip_package_version(word)
        } else {
            word.to_string()
        };
        if !token.contains('/') {
            return Ok(basename(&token).to_ascii_lowercase());
        }
        let candidates = self.joined_candidates(&token, state).ok_or(UNRESOLVED)?;
        for joined in &candidates {
            let lexical = lexically_normalized(joined);
            let real = real_path(joined);
            if state.created.iter().any(|created| {
                lexical.starts_with(created)
                    || real.as_ref().is_some_and(|real| real.starts_with(created))
            }) {
                return Err(UNRESOLVED);
            }
        }
        let real = candidates.first().and_then(|joined| real_path(joined));
        let base = real.as_deref().and_then(Path::file_name).map_or_else(
            || basename(&token).to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Ok(base.to_ascii_lowercase())
    }

    /// Absolute and `~` words ignore the working directory; relative words
    /// are joined to every candidate directory.
    fn joined_candidates(&self, word: &str, state: &ShellState) -> Option<Vec<PathBuf>> {
        self.joined_from(word, state.working_directories.as_deref())
    }

    /// `joined_candidates` against explicit base directories (`None` when
    /// they are unknown).
    fn joined_from(&self, word: &str, directories: Option<&[PathBuf]>) -> Option<Vec<PathBuf>> {
        let expanded = expand_home(word, &self.scope.home_dir);
        if expanded.is_absolute() {
            return Some(vec![expanded]);
        }
        directories.map(|directories| {
            directories
                .iter()
                .map(|directory| directory.join(&expanded))
                .collect()
        })
    }

    /// Output redirects are writes and input redirects are reads.
    fn check_redirects(&self, segment: &Segment, state: &ShellState) -> Verdict {
        for target in &segment.redirects {
            if !HARMLESS_REDIRECTS.contains(&target.as_str()) {
                self.check_shell_target(target, Effect::Write, state)?;
            }
        }
        for input in &segment.inputs {
            if !HARMLESS_REDIRECTS.contains(&input.as_str()) {
                self.check_shell_read(input, state)?;
            }
        }
        Ok(())
    }

    /// A word a command reads from: every path it can name, globs expanded
    /// the way `check_credential_word` expands them, must pass `check_read_path`.
    fn check_shell_read(&self, word: &str, state: &ShellState) -> Verdict {
        let pattern = brace_groups_as_wildcards(word).ok_or(UNRESOLVED)?;
        if is_unresolved_word(&pattern) {
            return Err(UNRESOLVED);
        }
        for joined in self.joined_candidates(&pattern, state).ok_or(UNRESOLVED)? {
            let matches = if has_glob(&pattern) {
                expand_glob(&joined).ok_or(UNRESOLVED)?
            } else {
                vec![joined]
            };
            for path in &matches {
                self.check_read_path(path, &state.created)?;
            }
        }
        Ok(())
    }

    fn check_shell_target(&self, word: &str, effect: Effect, state: &ShellState) -> Verdict {
        self.check_target_in(
            word,
            effect,
            state.working_directories.as_deref(),
            &state.created,
        )
    }

    /// `check_shell_target` with a relative word resolved against the given
    /// base directories instead of the shell's.
    fn check_target_in(
        &self,
        word: &str,
        effect: Effect,
        directories: Option<&[PathBuf]>,
        created: &[PathBuf],
    ) -> Verdict {
        if is_unresolved_word(word) {
            return Err(UNRESOLVED);
        }
        if has_glob(word) {
            // A glob may match a link. Only removal of the final component is
            // safe, because unlinking a link never follows it.
            let parent = word.rsplit_once('/').map_or("", |(parent, _)| parent);
            if effect == Effect::Write || has_glob(parent) {
                return Err(UNRESOLVED);
            }
            if effect == Effect::RemoveTree {
                let parent = if word.starts_with('/') && parent.is_empty() {
                    "/"
                } else {
                    parent
                };
                let parent = if parent.is_empty() { "." } else { parent };
                for joined in self.joined_from(parent, directories).ok_or(UNRESOLVED)? {
                    if self.is_protected(&real_path(&joined).ok_or(UNRESOLVED)?) {
                        return Err(RM_ROOT);
                    }
                }
            }
        }
        for joined in self.joined_from(word, directories).ok_or(UNRESOLVED)? {
            self.check_path(&joined, effect, created)?;
        }
        Ok(())
    }

    /// A file command's targets against the shell's working directories;
    /// git's against the directory `-C` moved it to.
    fn check_targets(
        &self,
        program: &str,
        args: &[String],
        targets: &[(String, Effect)],
        state: &ShellState,
    ) -> Verdict {
        let bases = if program == "git" {
            Some(self.git_bases(args, state)?)
        } else {
            None
        };
        for (word, effect) in targets {
            match &bases {
                Some(directories) => {
                    self.check_target_in(word, *effect, directories.as_deref(), &state.created)?;
                }
                None => self.check_shell_target(word, *effect, state)?,
            }
        }
        Ok(())
    }

    /// Git's base directories: every shell working directory moved by each
    /// `-C <path>` in order, a relative one against the previous. Relative
    /// `--git-dir`/`--work-tree` values and path operands resolve here, as
    /// `git.c` does after all its `chdir`s. A subcommand that writes needs the
    /// base itself in scope: `git -C /tmp commit` writes /tmp's repository.
    fn git_bases(
        &self,
        args: &[String],
        state: &ShellState,
    ) -> Result<Option<Vec<PathBuf>>, &'static str> {
        let invocation = git_invocation(args)?;
        let mut directories = state.working_directories.clone();
        for directory in &invocation.chdirs {
            if is_unresolved_word(directory) || has_glob(directory) {
                return Err(UNRESOLVED);
            }
            directories = self.joined_from(directory, directories.as_deref());
        }
        let reads = invocation
            .command
            .is_none_or(|index| git_only_reads(&args[index], &args[index + 1..]));
        if !reads {
            for base in directories.as_deref().ok_or(UNRESOLVED)? {
                self.check_path(base, Effect::Write, &state.created)?;
            }
        } else if !invocation.chdirs.is_empty() {
            // A read of another repository is still a read outside scope.
            for base in directories.as_deref().ok_or(UNRESOLVED)? {
                self.check_read_path(base, &state.created)?;
            }
        }
        Ok(directories)
    }

    /// Credential material named anywhere: arguments, option values,
    /// assignment values and input redirects.
    fn check_credential_word(&self, word: &str, state: &ShellState) -> Verdict {
        let value = word.split_once('=').map(|(_, value)| value);
        for candidate in std::iter::once(word).chain(value) {
            if candidate.is_empty() {
                continue;
            }
            if is_credential_path(candidate) {
                return Err(CREDENTIAL);
            }
            let pattern = brace_groups_as_wildcards(candidate).ok_or(UNRESOLVED)?;
            for joined in self.joined_candidates(&pattern, state).unwrap_or_default() {
                let matches = if has_glob(&pattern) {
                    expand_glob(&joined).ok_or(UNRESOLVED)?
                } else {
                    vec![joined]
                };
                if matches
                    .iter()
                    .any(|path| self.credential_at(path) == Some(true))
                {
                    return Err(CREDENTIAL);
                }
            }
        }
        Ok(())
    }

    fn record_created(&self, word: &str, state: &mut ShellState) {
        for joined in self.joined_candidates(word, state).unwrap_or_default() {
            if let Some(real) = real_path(&joined) {
                state.created.push(real);
            }
            state.created.push(lexically_normalized(&joined));
        }
    }

    /// Adds the directories a `cd`, `chdir`, `pushd` or `popd` may move to; every
    /// earlier candidate stays possible.
    fn change_directory(&self, program: &str, args: &[String], state: &mut ShellState) {
        if !DIRECTORY_CHANGES.contains(&program) {
            return;
        }
        let operands = operands(args);
        let next = match (program, operands.as_slice()) {
            _ if state.looping => None,
            ("cd" | "chdir", []) => Some("~"),
            // zsh's `cd old new` substitutes inside the current path, and `-`/`+N` read the
            // directory stack: only one operand is known, and only when it is anchored,
            // because a bare name may resolve through the owner's `CDPATH` or `cdablevars`.
            ("cd" | "chdir" | "pushd", [directory])
                if is_anchored_directory(directory)
                    && !is_unresolved_word(directory)
                    && !has_glob(directory) =>
            {
                Some(*directory)
            }
            _ => None,
        };
        let expanded = next.map(|directory| expand_home(directory, &self.scope.home_dir));
        state.working_directories = match (state.working_directories.take(), expanded) {
            (Some(mut directories), Some(expanded)) => {
                let moved: Vec<PathBuf> = directories
                    .iter()
                    .map(|base| base.join(&expanded))
                    .collect();
                for directory in moved {
                    if !directories.contains(&directory) {
                        directories.push(directory);
                    }
                }
                (directories.len() <= MAX_WORKING_DIRECTORIES).then_some(directories)
            }
            _ => None,
        };
    }
}

/// npx-style runners: the program token that follows is a package name that
/// may carry an `@version` or npm scope, stripped before matching.
const RUNNER_WRAPPERS: &[&str] = &["npx", "pnpx", "bunx", "corepack"];

/// Index of the program word and whether an `xargs` wrapper precedes it.
fn command_start(words: &[String]) -> Result<Wrapping<'_>, &'static str> {
    let mut index = 0;
    let mut via_xargs = false;
    let mut via_runner = false;
    let mut runner = None;
    let mut environment = None;
    while let Some(word) = words.get(index) {
        let word = command_word(word, environment == Some(index));
        if KEYWORDS.contains(&word) || is_assignment(word) {
            index += 1;
            continue;
        }
        if UNSUPPORTED_KEYWORDS.contains(&word) {
            return Err(UNPARSEABLE);
        }
        let name = basename(word).to_ascii_lowercase();
        // `bundle exec <cmd>` and `busybox <applet>` run the following program.
        if name == "bundle"
            && words
                .get(index + 1)
                .is_some_and(|next| basename(next).eq_ignore_ascii_case("exec"))
        {
            index += 2;
            continue;
        }
        if name == "busybox" || name == "toybox" {
            index += 1;
            continue;
        }
        // `uv run <cmd>`, `poetry run <cmd>` and the like run the following program.
        if let Some(tool) = PROJECT_RUNNERS.iter().find(|tool| tool.name == name) {
            let Some(command) = project_run_command(tool, words, index + 1)? else {
                break;
            };
            runner.get_or_insert(index);
            if tool.name == "hatch" {
                environment = Some(command);
            }
            index = command;
            continue;
        }
        let Some(wrapper) = WRAPPERS.iter().find(|wrapper| wrapper.name == name) else {
            break;
        };
        via_xargs |= name == "xargs";
        via_runner |= RUNNER_WRAPPERS.contains(&name.as_str());
        if OPAQUE_WRAPPERS.contains(&name.as_str()) {
            runner.get_or_insert(index);
        }
        index = wrapper.skip(words, index + 1)?;
    }
    let program = words
        .get(index)
        .map(|word| command_word(word, environment == Some(index)));
    // zsh's short `if`, `while` and `until` (`if [[ -n x ]] git push`) run
    // the words after a test's first `]]`: a test must end the segment there.
    if program == Some("[[")
        && words[index + 1..].iter().position(|word| word == "]]")
            != (words.len() - index).checked_sub(2)
    {
        return Err(UNPARSEABLE);
    }
    Ok(Wrapping {
        start: index,
        via_xargs,
        via_runner,
        runner,
        program,
    })
}

/// Index of the program a project tool's `run` runs, or `None` when the tool
/// runs another subcommand. Options before an unknown one cannot be told
/// apart from `run`'s operands, so a `run` anywhere after it fails closed.
fn project_run_command(
    runner: &Wrapper,
    words: &[String],
    index: usize,
) -> Result<Option<usize>, &'static str> {
    let subcommand = match runner.skip(words, index) {
        Ok(subcommand) => subcommand,
        Err(reason) if words[index..].iter().any(|word| word == "run") => return Err(reason),
        Err(_) => return Ok(None),
    };
    if words.get(subcommand).is_none_or(|word| word != "run") {
        return Ok(None);
    }
    let command = runner.skip(words, subcommand + 1)?;
    // hatch takes `+name=value` matrix filters (`-name=value` fails above).
    if runner.name == "hatch" && words.get(command).is_some_and(|word| word.starts_with('+')) {
        return Err(UNPARSEABLE);
    }
    Ok(Some(command))
}

/// The command a word names: after the `env:` of hatch's `env:command`.
fn command_word(word: &str, hatch_environment: bool) -> &str {
    if hatch_environment {
        word.split_once(':').map_or(word, |(_, command)| command)
    } else {
        word
    }
}

struct Wrapping<'a> {
    start: usize,
    via_xargs: bool,
    via_runner: bool,
    /// The first `xcrun`, `taskpolicy` or project runner unwrapped. Before
    /// these were unwrapped it was the program, and its verdicts still apply.
    runner: Option<usize>,
    /// The program word's text, without hatch's `env:` prefix.
    program: Option<&'a str>,
}

/// The scope's view of a program invocation, after wrapper stripping, name
/// case-folding, `@version`/scope stripping and symlink resolution.
fn strip_package_version(token: &str) -> String {
    if let Some(rest) = token.strip_prefix('@') {
        // `@scope/name[@ver]` runs the package's bin, usually named `name`.
        return match rest.split_once('/') {
            Some((_, name)) => name.split('@').next().unwrap_or(name).to_string(),
            None => rest.to_string(),
        };
    }
    token.split('@').next().unwrap_or(token).to_string()
}

/// The npm scope of `@scope/name`, which often names the tool (`@vercel/cli`).
fn package_scope(token: &str) -> Option<String> {
    let (scope, _) = token.strip_prefix('@')?.split_once('/')?;
    Some(scope.to_ascii_lowercase())
}
