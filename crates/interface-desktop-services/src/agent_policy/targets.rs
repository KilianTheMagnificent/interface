//! Per-program target extraction: the paths each command writes or reads, and
//! the git option and invocation readers.

use super::paths::Effect;
use super::risk::{CONTAINER_TOOLS, TAR_TOOLS, curl_transfer, git_config_risk, wget_transfer};
use super::shell::has_attached_value;
use super::{INTERPRETER, UNPARSEABLE, UNRESOLVED};

pub(super) fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

pub(super) fn is_short_cluster(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-') && !word.starts_with("--")
}

fn is_flag(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-')
}

/// Non-option words; every word after `--` is an operand.
pub(super) fn operands(args: &[String]) -> Vec<&str> {
    let mut found = Vec::new();
    let mut options = true;
    for word in args {
        if options && word == "--" {
            options = false;
        } else if !options || !is_flag(word) {
            found.push(word.as_str());
        }
    }
    found
}

/// Option words before `--`.
pub(super) fn options(args: &[String]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(String::as_str)
        .take_while(|word| *word != "--")
        .filter(|word| is_flag(word))
}

/// Programs a strict task may run that print or summarize the files named by
/// their operands, each with the options that take a separate value word.
const READ_OPERAND_PROGRAMS: &[(&str, &[&str])] = &[
    ("cat", &[]),
    ("head", &["-n", "-c"]),
    ("tail", &["-n", "-c", "-b"]),
    ("wc", &[]),
    ("cut", &["-b", "-c", "-d", "-f"]),
    ("du", &["-B", "-d", "-t"]),
    ("stat", &["-f", "-t"]),
    ("ls", &[]),
];

pub(super) fn read_operand_options(program: &str) -> Option<&'static [&'static str]> {
    READ_OPERAND_PROGRAMS
        .iter()
        .find(|(name, _)| *name == program)
        .map(|(_, valued)| *valued)
}

/// Words naming files a command reads: a reading program's operands and
/// `--option=value` values, and the sources a copy reads (every operand that is
/// not already a checked destination).
pub(super) fn read_operands<'a>(
    program: &str,
    args: &'a [String],
    targets: &[(String, Effect)],
) -> Vec<&'a str> {
    if let Some(valued) = read_operand_options(program) {
        let mut found = positionals(args, valued);
        found.extend(
            options(args)
                .filter(|word| word.starts_with("--"))
                .filter_map(|word| word.split_once('=').map(|(_, value)| value)),
        );
        return found;
    }
    if matches!(program, "cp" | "install" | "rsync" | "ditto") {
        return operands(args)
            .into_iter()
            .filter(|word| !targets.iter().any(|(target, _)| target == word))
            .collect();
    }
    Vec::new()
}

/// Positional words, skipping the value after each listed option.
pub(super) fn positionals<'a>(args: &'a [String], valued: &[&str]) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if valued.contains(&word) {
            words.next();
        } else if !is_flag(word) {
            found.push(word);
        }
    }
    found
}

/// The path operands each file command changes.
pub(super) fn file_targets<'a>(program: &str, args: &'a [String]) -> Vec<(&'a str, Effect)> {
    let operands = operands(args);
    let with = |words: Vec<&'a str>, effect: Effect| -> Vec<(&'a str, Effect)> {
        words.into_iter().map(|word| (word, effect)).collect()
    };
    let target_directory = || {
        let mut found = Vec::new();
        let mut words = args.iter().map(String::as_str);
        while let Some(word) = words.next() {
            if word == "-t" || word == "--target-directory" {
                found.extend(words.next());
            } else if let Some(value) = word.strip_prefix("--target-directory=") {
                found.push(value);
            }
        }
        found
    };
    let last_and_target_directory = || {
        let mut found = target_directory();
        found.extend(operands.last());
        found
    };
    match program {
        "rm" => {
            let recursive = options(args).any(|option| {
                option == "--recursive" || (is_short_cluster(option) && option.contains(['r', 'R']))
            });
            with(
                operands,
                if recursive {
                    Effect::RemoveTree
                } else {
                    Effect::Remove
                },
            )
        }
        "rmdir" | "unlink" | "shred" => with(operands, Effect::Remove),
        "touch" | "mkdir" | "tee" | "truncate" => with(operands, Effect::Write),
        "mv" => {
            let destination = target_directory();
            let mut found = with(operands.clone(), Effect::Move);
            if destination.is_empty() {
                // The last operand is the destination, which is written, not moved.
                if let Some(last) = found.last_mut() {
                    last.1 = Effect::Write;
                }
            }
            found.extend(with(destination, Effect::Write));
            found
        }
        "sed" | "gsed" => with(sed_in_place_files(args), Effect::Write),
        "rsync" if args.iter().any(|word| word == "--remove-source-files") => {
            with(operands, Effect::Write)
        }
        // A hard link shares its source's content, so every operand must be in scope.
        "ln" if !options(args).any(|option| {
            option == "--symbolic" || (is_short_cluster(option) && option.contains('s'))
        }) =>
        {
            with(operands, Effect::Write)
        }
        "cp" if options(args).any(|option| {
            option == "--link" || (is_short_cluster(option) && option.contains('l'))
        }) =>
        {
            with(operands, Effect::Write)
        }
        "cp" | "ln" | "install" | "rsync" | "ditto" => {
            with(last_and_target_directory(), Effect::Write)
        }
        program if CONTAINER_TOOLS.contains(&program) => {
            with(bind_mount_sources(args), Effect::Write)
        }
        "chmod" | "chown" | "chgrp" => {
            let reference = options(args).any(|option| option.starts_with("--reference"));
            let skip = usize::from(!reference);
            with(operands.into_iter().skip(skip).collect(), Effect::Write)
        }
        "curl" => with(curl_transfer(args).outputs, Effect::Write),
        "wget" | "wget2" => with(wget_transfer(args).outputs, Effect::Write),
        "find" => with(find_targets(args), Effect::Write),
        // Extractors and patchers write under a destination directory.
        program if TAR_TOOLS.contains(&program) => {
            with(flag_values(args, &["-C", "--directory"]), Effect::Write)
        }
        "unzip" => with(flag_values(args, &["-d"]), Effect::Write),
        "7z" | "7za" | "7zz" => with(dash_o_values(args), Effect::Write),
        "patch" => with(
            flag_values(args, &["-d", "--directory", "-o", "--output"]),
            Effect::Write,
        ),
        // In-place editors, like `sed -i`.
        "perl" | "ruby" => with(inline_edit_files(args), Effect::Write),
        // Line editors write their file operand.
        "vim" | "vi" | "nvim" | "view" | "ed" | "ex" => with(editor_files(args), Effect::Write),
        // `git clone <url> <dest>` and `--work-tree`/`--git-dir` write outside the repo.
        "git" => with(git_write_targets(args), Effect::Write),
        // Global install prefixes escape scope.
        "npm" | "pnpm" | "yarn" | "bun" => with(flag_values(args, &["--prefix"]), Effect::Write),
        "pip" | "pip3" => with(
            flag_values(args, &["-t", "--target", "--prefix", "--root"]),
            Effect::Write,
        ),
        _ => Vec::new(),
    }
}

/// Values of the given options (`-C x`, `-C=x`, `-Cx` for shorts, `--dir=x`).
fn flag_values<'a>(args: &'a [String], flags: &[&str]) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if flags.contains(&word) {
            found.extend(words.next());
        } else if let Some(value) = flags.iter().find_map(|flag| {
            if flag.starts_with("--") {
                word.strip_prefix(flag)
                    .and_then(|rest| rest.strip_prefix('='))
            } else {
                word.strip_prefix(flag).filter(|rest| !rest.is_empty())
            }
        }) {
            found.push(value);
        }
    }
    found
}

/// 7-Zip writes to `-o<dir>` (the value is always attached).
fn dash_o_values(args: &[String]) -> Vec<&str> {
    args.iter()
        .filter_map(|word| word.strip_prefix("-o").filter(|rest| !rest.is_empty()))
        .collect()
}

/// `perl -i` / `ruby -i` rewrite the file operands, skipping `-e`/`-E` scripts.
fn inline_edit_files(args: &[String]) -> Vec<&str> {
    let in_place = options(args)
        .any(|option| is_short_cluster(option) && option.contains('i') || option == "-i");
    if !in_place {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(word, "-e" | "-E") || (is_short_cluster(word) && word.contains('e')) {
            words.next();
        } else if !word.starts_with('-') {
            found.push(word);
        }
    }
    found
}

/// The file operands an editor writes, skipping `-c`/`--cmd` scripts.
fn editor_files(args: &[String]) -> Vec<&str> {
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(
            word,
            "-c" | "--cmd" | "-u" | "-i" | "-U" | "--startuptime" | "-w" | "-W" | "-s"
        ) {
            words.next();
        } else if !word.starts_with('-') {
            found.push(word);
        }
    }
    found
}

/// `git worktree add <path>`, `move <worktree> <new-path>` and `remove <worktree>`
/// create, move or delete a checkout wherever the path points. `rest` is
/// everything after `worktree`.
fn git_worktree_targets(rest: &[String]) -> Vec<&str> {
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    let Some(action) = words.iter().position(|word| !word.starts_with('-')) else {
        return Vec::new();
    };
    let mut operands = Vec::new();
    let mut items = words[action + 1..].iter().copied();
    while let Some(word) = items.next() {
        if matches!(word, "-b" | "-B" | "--reason") {
            items.next();
        } else if !word.starts_with('-') {
            operands.push(word);
        }
    }
    match words[action] {
        "add" | "remove" => operands.into_iter().take(1).collect(),
        "move" => operands.into_iter().skip(1).take(1).collect(),
        _ => Vec::new(),
    }
}

/// Shortest git long-option abbreviation the policy treats as its full option.
/// Git resolves any unambiguous prefix, and two characters is where its own
/// ambiguity refusals give way to real matches for the options that matter here
/// (`--op` is `--open-files-in-pager`); a shorter prefix would be over-broad.
const MIN_GIT_ABBREV: usize = 2;

/// The name of a long-option word: `--name` or `--name=value` → `name`.
fn long_option_name(word: &str) -> Option<&str> {
    let rest = word.strip_prefix("--").filter(|rest| !rest.is_empty())?;
    Some(rest.split('=').next().unwrap_or(rest))
}

/// Whether `word` is a long option whose name (at least [`MIN_GIT_ABBREV`]
/// characters) is a prefix of `full`, the way git resolves an abbreviation.
/// Fail-closed: it may also match a name git would call ambiguous.
fn abbreviates(word: &str, full: &str) -> bool {
    long_option_name(word).is_some_and(|name| {
        name.len() >= MIN_GIT_ABBREV && full.len() >= name.len() && full.starts_with(name)
    })
}

/// Whether any word abbreviates any of the full option names.
pub(super) fn any_option_abbreviates(rest: &[String], fulls: &[&str]) -> bool {
    rest.iter()
        .any(|word| fulls.iter().any(|full| abbreviates(word, full)))
}

/// Values of git long options named by `fulls`, matching git's prefix
/// abbreviation for both the attached (`--work-tr=x`) and separate
/// (`--work-tree x`) forms.
fn git_option_values<'a>(args: &'a [String], fulls: &[&str]) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        let attached = word.contains('=');
        if fulls.iter().any(|full| abbreviates(word, full)) {
            if attached {
                found.extend(word.split_once('=').map(|(_, value)| value));
            } else {
                found.extend(words.next());
            }
        }
    }
    found
}

/// Paths a git invocation writes outside its repository, all resolved
/// against the base `-C` moved git to: `--git-dir`, `--work-tree`,
/// `--separate-git-dir`, `--output` and `--output-directory` values, a clone
/// or init destination, worktree paths, `format-patch`/`archive` `-o`
/// outputs and a bundle file.
fn git_write_targets(args: &[String]) -> Vec<&str> {
    const CLONE_VALUED: &[&str] = &[
        "-b",
        "--branch",
        "-o",
        "--origin",
        "-u",
        "--upload-pack",
        "--template",
        "--reference",
        "--reference-if-able",
        "--depth",
        "--shallow-since",
        "--shallow-exclude",
        "-j",
        "--jobs",
        "--filter",
        "--separate-git-dir",
        "-c",
        "--config",
        "--bundle-uri",
        "--ref-format",
        "--server-option",
    ];
    const INIT_VALUED: &[&str] = &[
        "--template",
        "--separate-git-dir",
        "-b",
        "--initial-branch",
        "--object-format",
        "--ref-format",
    ];
    let mut found = git_option_values(
        args,
        &[
            "work-tree",
            "git-dir",
            "separate-git-dir",
            "output",
            "output-directory",
        ],
    );
    let Ok(GitInvocation {
        command: Some(index),
        ..
    }) = git_invocation(args)
    else {
        return found;
    };
    let rest = &args[index + 1..];
    match args[index].as_str() {
        "worktree" => found.extend(git_worktree_targets(rest)),
        // clone: <url> [<dest>]; init: [<dir>]. Every later operand is a
        // target too, so an unlisted valued option cannot hide the destination.
        "clone" => found.extend(positionals(rest, CLONE_VALUED).into_iter().skip(1)),
        "init" => found.extend(positionals(rest, INIT_VALUED)),
        "format-patch" | "archive" => found.extend(flag_values(rest, &["-o"])),
        "bundle" if rest.first().is_some_and(|word| word == "create") => {
            found.extend(operands(&rest[1..]).first());
        }
        _ => {}
    }
    found
}

/// Git's global options, read the way `git.c` reads them.
pub(super) struct GitInvocation<'a> {
    /// `-C <path>` values in the order git applies them.
    pub(super) chdirs: Vec<&'a str>,
    /// Index of the subcommand; `None` when git only prints help or a version.
    pub(super) command: Option<usize>,
}

/// A `-C` needs a value; `-C<dir>` (which git rejects today) is read as that
/// chdir so a git that accepts it is still modeled. `-c`, `--config-env` and
/// `--exec-path=` name configuration or programs git obeys; any other unknown
/// option is unparseable.
pub(super) fn git_invocation(args: &[String]) -> Result<GitInvocation<'_>, &'static str> {
    const VALUED: &[&str] = &["--git-dir", "--work-tree", "--namespace", "--super-prefix"];
    const FLAGS: &[&str] = &[
        "-p",
        "-P",
        "--paginate",
        "--no-pager",
        "--bare",
        "--exec-path",
        "--html-path",
        "--man-path",
        "--info-path",
        "--no-replace-objects",
        "--literal-pathspecs",
        "--glob-pathspecs",
        "--noglob-pathspecs",
        "--icase-pathspecs",
        "--no-optional-locks",
        "--no-lazy-fetch",
        "--no-advice",
        "--version",
        "--help",
        "-h",
        "-v",
    ];
    let mut chdirs = Vec::new();
    let mut index = 0;
    while let Some(word) = args.get(index).map(String::as_str) {
        if word == "-C" {
            chdirs.push(args.get(index + 1).map(String::as_str).ok_or(UNRESOLVED)?);
            index += 2;
        } else if let Some(attached) = word.strip_prefix("-C").filter(|rest| !rest.is_empty()) {
            chdirs.push(attached);
            index += 1;
        } else if VALUED.contains(&word) {
            index += 2;
        } else if FLAGS.contains(&word) || VALUED.iter().any(|flag| has_attached_value(word, flag))
        {
            index += 1;
        } else if word.starts_with("-c")
            || word.starts_with("--config-env")
            || word.starts_with("--exec-path=")
        {
            // Configuration overrides can define aliases, pagers and ssh commands.
            return Err(INTERPRETER);
        } else if word.starts_with('-') {
            return Err(UNPARSEABLE);
        } else {
            break;
        }
    }
    Ok(GitInvocation {
        chdirs,
        command: (index < args.len()).then_some(index),
    })
}

/// Subcommands that never write the repository or its working tree, so the
/// directory git runs in need not be in scope. Anything else, including a
/// subcommand this table does not know, counts as a write.
pub(super) fn git_only_reads(command: &str, rest: &[String]) -> bool {
    let action = rest
        .iter()
        .map(String::as_str)
        .find(|word| !word.starts_with('-'));
    match command {
        "status" | "log" | "diff" | "show" | "rev-parse" | "ls-files" | "ls-tree" | "ls-remote"
        | "cat-file" | "blame" | "grep" | "shortlog" | "describe" | "name-rev" | "rev-list"
        | "merge-base" | "diff-tree" | "diff-files" | "diff-index" | "show-ref"
        | "for-each-ref" | "check-ignore" | "check-attr" | "check-ref-format" | "count-objects"
        | "whatchanged" | "cherry" | "range-diff" | "show-branch" | "verify-commit"
        | "verify-tag" | "var" | "version" | "help" => true,
        "config" => git_config_risk(rest).is_none(),
        "branch" | "tag" => git_lists_refs(command, rest),
        "worktree" => action == Some("list"),
        "stash" => matches!(action, Some("list" | "show")),
        "remote" => matches!(action, None | Some("show" | "get-url")),
        "reflog" => matches!(action, None | Some("show" | "exists")),
        "submodule" => matches!(action, None | Some("status" | "summary")),
        "notes" => matches!(action, None | Some("list" | "show")),
        "bundle" => matches!(action, Some("verify" | "list-heads")),
        _ => false,
    }
}

/// `git branch`/`git tag` only list when no option creates, renames, deletes
/// or annotates and either `--list`/`-l` is given (operands are patterns) or
/// there are no operands at all.
fn git_lists_refs(command: &str, rest: &[String]) -> bool {
    let (write_letters, write_flags, valued): (&str, &[&str], &[&str]) = if command == "branch" {
        (
            "dDmMcCfut",
            &[
                "--delete",
                "--move",
                "--copy",
                "--force",
                "--set-upstream-to",
                "--unset-upstream",
                "--edit-description",
                "--track",
                "--create-reflog",
            ],
            &[
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
                "--color",
                "--abbrev",
                "--column",
            ],
        )
    } else {
        (
            "dasumFfe",
            &[
                "--delete",
                "--annotate",
                "--sign",
                "--local-user",
                "--message",
                "--file",
                "--force",
                "--edit",
                "--cleanup",
                "--create-reflog",
            ],
            &[
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
                "--color",
                "--column",
            ],
        )
    };
    let mut listing = false;
    let mut operands = false;
    let mut words = rest.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--list" {
            listing = true;
        } else if word.starts_with("--") {
            let name = word.split('=').next().unwrap_or(word);
            if write_flags.contains(&name) {
                return false;
            }
            if valued.contains(&name) && !word.contains('=') {
                words.next();
            }
        } else if is_short_cluster(word) {
            if word[1..].contains(|letter| write_letters.contains(letter)) {
                return false;
            }
            if word[1..].contains('l') {
                listing = true;
            }
        } else {
            operands = true;
        }
    }
    listing || !operands
}

/// Files `sed -i` rewrites: every operand but the script, skipping the empty
/// or `.ext` backup suffix BSD `sed -i` takes as a separate word.
fn sed_in_place_files(args: &[String]) -> Vec<&str> {
    let in_place = options(args).any(|option| {
        option.starts_with("--in-place") || (is_short_cluster(option) && option.contains('i'))
    });
    if !in_place {
        return Vec::new();
    }
    let scripted = options(args).any(|option| {
        matches!(option, "-e" | "-f" | "--expression" | "--file")
            || option.starts_with("--expression=")
            || option.starts_with("--file=")
    });
    operands(args)
        .into_iter()
        .filter(|word| !word.is_empty())
        .skip(usize::from(!scripted))
        .collect()
}

/// Host paths bind-mounted into a container, which may write through them.
fn bind_mount_sources(args: &[String]) -> Vec<&str> {
    let is_host_path = |path: &&str| path.starts_with(['/', '.', '~']);
    let mut found = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        let (name, inline) = word
            .split_once('=')
            .map_or((word, None), |(name, value)| (name, Some(value)));
        let value = match name {
            "-v" | "--volume" | "--mount" => inline.or_else(|| words.next()),
            _ => word.strip_prefix("-v").filter(|rest| !rest.is_empty()),
        };
        let Some(value) = value else { continue };
        if name == "--mount" {
            found.extend(
                value
                    .split(',')
                    .filter_map(|field| {
                        field
                            .strip_prefix("source=")
                            .or_else(|| field.strip_prefix("src="))
                    })
                    .filter(is_host_path),
            );
        } else {
            found.extend(value.split(':').next().filter(is_host_path));
        }
    }
    found
}

/// `-delete` makes the start paths write targets; `-fprint*` and `-fls` name output files.
fn find_targets(args: &[String]) -> Vec<&str> {
    let mut found = Vec::new();
    if args.iter().any(|word| word == "-delete") {
        found.extend(
            args.iter()
                .map(String::as_str)
                .take_while(|word| !word.starts_with('-') && *word != "(" && *word != "!"),
        );
    }
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(word, "-fprint" | "-fprint0" | "-fprintf" | "-fls") {
            found.extend(words.next());
        }
    }
    found
}
