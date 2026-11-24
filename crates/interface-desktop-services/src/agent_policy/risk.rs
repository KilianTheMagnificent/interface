//! Program risk tables: programs and subcommands that always need the owner,
//! whatever their paths.

use super::targets::{
    any_option_abbreviates, git_invocation, is_short_cluster, operands, options, positionals,
};
use super::{AGENT_CONFIG, CREDENTIAL, INTERPRETER, OUTSIDE};

/// Container CLIs that share docker's `run -v/--mount` host bind syntax.
pub(super) const CONTAINER_TOOLS: &[&str] = &["docker", "podman", "nerdctl"];
/// tar front ends that share `-C/--directory` extraction targets.
pub(super) const TAR_TOOLS: &[&str] = &["tar", "bsdtar", "gtar"];

/// A program whose risk depends on its arguments.
pub(super) fn has_argument_checks(program: &str) -> bool {
    SHELLS.contains(&program)
        || CONTAINER_TOOLS.contains(&program)
        || TAR_TOOLS.contains(&program)
        || DEPLOY_TOOLS
            .iter()
            .any(|tool| tool.names.contains(&program))
        || is_deploy_verb_tool(program)
        || matches!(
            program,
            "git"
                | "gh"
                | "glab"
                | "curl"
                | "wget"
                | "npm"
                | "yarn"
                | "pnpm"
                | "bun"
                | "cargo"
                | "gem"
                | "twine"
                | "go"
                | "brew"
                | "defaults"
                | "rsync"
                | "find"
                | "fd"
                | "fdfind"
                | "openssl"
                | "pip"
                | "pip3"
                | "pipx"
                | "uv"
                | "rustup"
                | "wget2"
                | "grpcurl"
                | "buildah"
                | "skopeo"
                | "oras"
                | "crane"
                | "poetry"
                | "pdm"
                | "hatch"
                | "flit"
                | "mvn"
                | "gradle"
                | "gradlew"
                | "dotnet"
                | "tmux"
                | "screen"
                | "leaks"
                | "xctrace"
                | "gio"
                | "plutil"
                | "unzip"
                | "7z"
                | "7za"
                | "7zz"
                | "patch"
                | "perl"
                | "ruby"
                | "vim"
                | "vi"
                | "nvim"
                | "ed"
                | "ex"
                | "view"
        )
}

/// Programs that always need the owner, whatever their arguments.
const RISKY_PROGRAMS: &[(&str, &str)] = &[
    ("sudo", "risky:system"),
    ("su", "risky:system"),
    ("doas", "risky:system"),
    ("ssh", "risky:system"),
    ("slogin", "risky:system"),
    ("ssh-copy-id", "risky:system"),
    ("mosh", "risky:system"),
    ("autossh", "risky:system"),
    ("sshpass", "risky:system"),
    ("scp", "risky:system"),
    ("sftp", "risky:system"),
    ("mount", "risky:system"),
    ("umount", "risky:system"),
    ("sshfs", "risky:system"),
    ("dscl", "risky:system"),
    ("nvram", "risky:system"),
    ("installer", "risky:system"),
    ("softwareupdate", "risky:system"),
    ("tccutil", "risky:system"),
    ("visudo", "risky:system"),
    ("sudoedit", "risky:system"),
    ("plistbuddy", "risky:system"),
    ("security", "risky:system"),
    ("launchctl", "risky:system"),
    ("osascript", "risky:system"),
    ("open", "risky:system"),
    ("xdg-open", "risky:system"),
    ("crontab", "risky:system"),
    ("pmset", "risky:system"),
    ("networksetup", "risky:system"),
    ("systemsetup", "risky:system"),
    ("diskutil", "risky:system"),
    ("dd", "risky:system"),
    ("shutdown", "risky:system"),
    ("reboot", "risky:system"),
    ("halt", "risky:system"),
    ("mail", "risky:send"),
    ("mailx", "risky:send"),
    ("swaks", "risky:send"),
    ("mutt", "risky:send"),
    ("neomutt", "risky:send"),
    ("sendmail", "risky:send"),
    ("lftp", "risky:send"),
    ("ncftp", "risky:send"),
    ("ftpput", "risky:send"),
    ("tftp", "risky:send"),
    ("wscat", "risky:send"),
    ("mosquitto_pub", "risky:send"),
    ("mosquitto_sub", "risky:send"),
    ("msmtp", "risky:send"),
    ("nc", "risky:send"),
    ("ncat", "risky:send"),
    ("netcat", "risky:send"),
    ("telnet", "risky:send"),
    ("ftp", "risky:send"),
    ("aws", "risky:cloud"),
    ("gcloud", "risky:cloud"),
    ("gsutil", "risky:cloud"),
    ("az", "risky:cloud"),
    ("stripe", "risky:cloud"),
    ("eval", INTERPRETER),
    ("exec", INTERPRETER),
    ("expect", INTERPRETER),
    ("at", INTERPRETER),
    ("batch", INTERPRETER),
    ("source", INTERPRETER),
    (".", INTERPRETER),
    ("alias", INTERPRETER),
    ("trap", INTERPRETER),
    ("watch", INTERPRETER),
    ("parallel", INTERPRETER),
    ("script", INTERPRETER),
    ("flock", INTERPRETER),
    ("lockf", INTERPRETER),
    ("entr", INTERPRETER),
    ("unbuffer", INTERPRETER),
    ("bind", INTERPRETER),
    ("complete", INTERPRETER),
    ("compgen", INTERPRETER),
    ("enable", INTERPRETER),
    ("fc", INTERPRETER),
    ("mapfile", INTERPRETER),
    ("readarray", INTERPRETER),
    ("emulate", INTERPRETER),
    ("zmodload", INTERPRETER),
    ("sched", INTERPRETER),
    ("zpty", INTERPRETER),
    ("zargs", INTERPRETER),
    ("strace", INTERPRETER),
    ("ltrace", INTERPRETER),
    ("dtruss", INTERPRETER),
    ("valgrind", INTERPRETER),
    ("gdb", INTERPRETER),
    ("lldb", INTERPRETER),
    ("sandbox-exec", INTERPRETER),
    ("ztcp", "risky:send"),
    ("zftp", "risky:send"),
    ("zsocket", "risky:send"),
    ("socat", "risky:send"),
    ("pkexec", "risky:system"),
    ("chroot", "risky:system"),
    ("ssh-agent", "risky:system"),
    ("ssh-add", "risky:system"),
    ("op", "risky:system"),
    ("ngrok", "risky:system"),
    ("cloudflared", "risky:system"),
    ("tailscale", "risky:system"),
    ("rclone", "risky:cloud"),
    ("s3cmd", "risky:cloud"),
    ("azcopy", "risky:cloud"),
    ("http", "risky:curl-upload"),
    ("https", "risky:curl-upload"),
    ("xh", "risky:curl-upload"),
    ("xhs", "risky:curl-upload"),
];

/// Shells run a script operand like any other script in scope, but a command
/// string, stdin or an interactive session hides what runs.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "ksh", "csh", "tcsh", "pwsh",
];

/// `(group, read-only actions)` for the GitHub CLI; `*` allows any action.
const GH_READS: &[(&str, &[&str])] = &[
    ("pr", &["view", "list", "status", "diff", "checks"]),
    ("issue", &["view", "list", "status"]),
    ("release", &["view", "list"]),
    ("run", &["view", "list", "watch"]),
    ("workflow", &["view", "list"]),
    ("repo", &["view", "list"]),
    ("gist", &["view", "list"]),
    ("label", &["list"]),
    ("cache", &["list"]),
    ("search", &["*"]),
    ("status", &["*"]),
    ("help", &["*"]),
    ("version", &["*"]),
];

const GLAB_READS: &[(&str, &[&str])] = &[
    ("mr", &["view", "list", "diff"]),
    ("issue", &["view", "list"]),
    ("release", &["view", "list"]),
    ("ci", &["view", "list", "status"]),
    ("pipeline", &["view", "list", "status"]),
    ("repo", &["view", "list"]),
    ("label", &["list"]),
    ("help", &["*"]),
    ("version", &["*"]),
];

/// A deploy or infrastructure CLI: only these first subcommands run unasked.
struct DeployTool {
    names: &'static [&'static str],
    valued: &'static [&'static str],
    reads: &'static [&'static str],
    /// The bare command deploys.
    bare_deploys: bool,
}

const DEPLOY_TOOLS: &[DeployTool] = &[
    DeployTool {
        names: &["kubectl"],
        valued: &[
            "-n",
            "--namespace",
            "--context",
            "--kubeconfig",
            "--cluster",
            "--user",
            "-s",
            "--server",
        ],
        reads: &[
            "get",
            "describe",
            "logs",
            "top",
            "explain",
            "version",
            "api-resources",
            "api-versions",
            "cluster-info",
            "diff",
            "wait",
        ],
        bare_deploys: false,
    },
    DeployTool {
        names: &["helm"],
        valued: &["-n", "--namespace", "--kube-context", "--kubeconfig"],
        reads: &[
            "list", "ls", "status", "get", "show", "search", "template", "lint", "version",
            "history", "env",
        ],
        bare_deploys: false,
    },
    DeployTool {
        names: &["terraform", "tofu"],
        valued: &[],
        reads: &[
            "plan",
            "validate",
            "fmt",
            "init",
            "show",
            "output",
            "providers",
            "version",
            "graph",
            "get",
            "help",
        ],
        bare_deploys: false,
    },
    DeployTool {
        names: &["pulumi"],
        valued: &["-C", "--cwd", "-s", "--stack"],
        reads: &["preview", "version", "about", "whoami", "logs", "help"],
        bare_deploys: false,
    },
    DeployTool {
        names: &["vercel", "vc"],
        valued: &[
            "--scope",
            "--token",
            "-t",
            "--cwd",
            "-A",
            "--local-config",
            "-Q",
            "--global-config",
            "-S",
        ],
        reads: &[
            "ls", "list", "inspect", "logs", "whoami", "help", "dev", "build",
        ],
        bare_deploys: true,
    },
    DeployTool {
        names: &["netlify"],
        valued: &["--cwd", "--filter"],
        reads: &[
            "status",
            "logs",
            "help",
            "dev",
            "build",
            "sites:list",
            "watch",
        ],
        bare_deploys: false,
    },
    DeployTool {
        names: &["fly", "flyctl"],
        valued: &["-a", "--app", "-c", "--config"],
        reads: &["status", "logs", "version", "help", "doctor", "releases"],
        bare_deploys: false,
    },
    DeployTool {
        names: &["heroku"],
        valued: &["-a", "--app", "-r", "--remote"],
        reads: &[
            "logs",
            "ps",
            "info",
            "help",
            "version",
            "releases",
            "status",
            "apps",
            "whoami",
            "auth:whoami",
        ],
        bare_deploys: false,
    },
    DeployTool {
        names: &["firebase"],
        valued: &["-P", "--project"],
        reads: &["help", "projects:list", "emulators:start", "serve"],
        bare_deploys: false,
    },
];

const BREW_READS: &[&str] = &[
    "list", "ls", "info", "search", "outdated", "config", "doctor", "deps", "leaves", "desc",
    "uses", "commands", "help", "shellenv",
];
const BREW_PATH_QUERIES: &[&str] = &[
    "--prefix",
    "--cellar",
    "--repository",
    "--cache",
    "--version",
    "--env",
];

const PACKAGE_PUBLISH_VERBS: &[&str] = &[
    "publish",
    "unpublish",
    "deprecate",
    "dist-tag",
    "owner",
    "adduser",
    "login",
    "logout",
    "token",
];
const PACKAGE_INSTALL_VERBS: &[&str] = &[
    "install",
    "i",
    "add",
    "update",
    "up",
    "upgrade",
    "uninstall",
    "remove",
    "rm",
    "un",
    "link",
    "ln",
];

pub(super) fn risky(program: &str, args: &[String]) -> Option<&'static str> {
    if let Some((_, reason)) = RISKY_PROGRAMS.iter().find(|(name, _)| *name == program) {
        return Some(reason);
    }
    if SHELLS.contains(&program) {
        return args
            .first()
            .is_none_or(|first| first.starts_with('-'))
            .then_some(INTERPRETER);
    }
    if let Some(tool) = DEPLOY_TOOLS
        .iter()
        .find(|tool| tool.names.contains(&program))
    {
        let allowed = positionals(args, tool.valued)
            .first()
            .map_or(!tool.bare_deploys, |first| tool.reads.contains(first));
        return (!allowed).then_some("risky:deploy");
    }
    if let Some(reason) = deploy_verb_risk(program, args) {
        return Some(reason);
    }
    match program {
        "git" => git_risk(args),
        "gh" => hub_risk(args, GH_READS, "risky:gh"),
        "glab" => hub_risk(args, GLAB_READS, "risky:glab"),
        "curl" => curl_transfer(args).upload.then_some("risky:curl-upload"),
        "wget" | "wget2" => wget_transfer(args).upload.then_some("risky:curl-upload"),
        "grpcurl" => args
            .iter()
            .any(|word| matches!(word.as_str(), "-d" | "--data") || word.starts_with("-d"))
            .then_some("risky:send"),
        "gio" => (positionals(args, &[]).first() == Some(&"open")).then_some("risky:system"),
        "tmux" => positionals(args, &[])
            .first()
            .is_some_and(|first| {
                matches!(
                    *first,
                    "send-keys" | "run-shell" | "run" | "respawn-pane" | "respawn-window"
                )
            })
            .then_some(INTERPRETER),
        "screen" => options(args)
            .any(|option| option == "-X")
            .then_some(INTERPRETER),
        "leaks" | "xctrace" if profiler_launches(program, args) => Some(INTERPRETER),
        "plutil" => options(args)
            .any(|option| {
                matches!(
                    option,
                    "-insert" | "-replace" | "-remove" | "-convert" | "-o"
                )
            })
            .then_some("risky:system"),
        "awk" | "gawk" | "mawk" | "nawk" => awk_executes(args).then_some(INTERPRETER),
        "sed" | "gsed" => sed_executes(args).then_some(INTERPRETER),
        "sqlite3" => args
            .iter()
            .any(|word| {
                word.contains(".shell")
                    || word.contains(".system")
                    || word.contains(".import")
                    || word.contains(".output")
            })
            .then_some(INTERPRETER),
        "poetry" | "pdm" | "hatch" | "flit" | "mvn" | "gradle" | "gradlew" | "dotnet" => {
            publish_tool_risk(program, args)
        }
        "npm" | "yarn" | "pnpm" | "bun" => package_manager_risk(program, args),
        "cargo" | "gem" | "twine" | "go" | "buildah" | "skopeo" | "oras" | "crane" => {
            publisher_risk(program, args)
        }
        tool if CONTAINER_TOOLS.contains(&tool) => publisher_risk(tool, args),
        "brew" => {
            let first = positionals(args, &[]).first().copied();
            let allowed = first.is_none_or(|first| BREW_READS.contains(&first))
                || args
                    .first()
                    .is_some_and(|word| BREW_PATH_QUERIES.contains(&word.as_str()));
            (!allowed).then_some("risky:install")
        }
        "defaults" => positionals(args, &[])
            .first()
            .is_some_and(|first| !["read", "read-type", "domains", "find", "help"].contains(first))
            .then_some("risky:system"),
        "rsync" => rsync_is_remote(args).then_some("risky:system"),
        "openssl" => (positionals(args, &[]).first() == Some(&"s_client")).then_some("risky:send"),
        "uv" => publish_tool_risk(program, args).or_else(|| toolchain_risk(program, args)),
        "pip" | "pip3" | "pipx" | "rustup" => toolchain_risk(program, args),
        "find" => args
            .iter()
            .any(|word| matches!(word.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
            .then_some(INTERPRETER),
        "fd" | "fdfind" => args
            .iter()
            .any(|word| matches!(word.as_str(), "-x" | "--exec" | "-X" | "--exec-batch"))
            .then_some(INTERPRETER),
        _ => None,
    }
}

/// macOS profilers that launch the command after `--` (`leaks --atExit`,
/// `xctrace record --launch`).
fn profiler_launches(program: &str, args: &[String]) -> bool {
    let launch = if program == "leaks" {
        "--atExit"
    } else {
        "--launch"
    };
    args.iter().any(|word| word == launch)
}

fn git_risk(args: &[String]) -> Option<&'static str> {
    let invocation = match git_invocation(args) {
        Ok(invocation) => invocation,
        Err(reason) => return Some(reason),
    };
    let index = invocation.command?;
    let command = args[index].as_str();
    let rest = &args[index + 1..];
    let has = |flag: &str| rest.iter().any(|word| word == flag);
    let short = |letter: char| {
        rest.iter()
            .any(|word| is_short_cluster(word) && word[1..].contains(letter))
    };
    match command {
        "push" | "send-pack" | "http-push" | "http-backend" => Some("risky:git-push"),
        "svn" if has("dcommit") => Some("risky:git-push"),
        "p4" if has("submit") => Some("risky:git-push"),
        "hook" => Some(INTERPRETER),
        "reset" if has("--hard") => Some("risky:git-reset-hard"),
        "clean" if short('f') || has("--force") => Some("risky:git-clean"),
        "branch"
            if short('D')
                || ((short('d') || has("--delete")) && (short('f') || has("--force"))) =>
        {
            Some("risky:git-branch-delete")
        }
        "tag" if short('d') || has("--delete") => Some("risky:git-tag-delete"),
        "lfs" | "subtree" if has("push") => Some("risky:git-push"),
        "credential" | "credential-cache" | "credential-store" | "credential-osxkeychain" => {
            Some(CREDENTIAL)
        }
        "send-email" => Some("risky:send"),
        "config" => git_config_risk(rest),
        "init" | "clone" if any_option_abbreviates(rest, &["template"]) => Some(AGENT_CONFIG),
        "clone"
            if rest
                .iter()
                .any(|word| word == "-u" || word.starts_with("-c"))
                || any_option_abbreviates(rest, &["config"]) =>
        {
            Some(INTERPRETER)
        }
        // `--upload-pack`/`--receive-pack` (and the `--exec` alias transport
        // commands take) name a program git runs on the far side.
        _ if any_option_abbreviates(rest, &["upload-pack", "receive-pack", "exec"]) => {
            Some(INTERPRETER)
        }
        "submodule" if has("foreach") => Some(INTERPRETER),
        "bisect" if has("run") => Some(INTERPRETER),
        // `-x <cmd>` runs a command between rebased commits.
        "rebase" if short('x') => Some(INTERPRETER),
        // `-O[<pager>]` opens the matching files in a program.
        "grep"
            if rest.iter().any(|word| sets_short_flag(word, 'O', "efABCm"))
                || any_option_abbreviates(rest, &["open-files-in-pager"]) =>
        {
            Some(INTERPRETER)
        }
        // `-x`/`--extcmd` run a command in place of the diff tool; `--tool` names a known one.
        "difftool" | "mergetool"
            if rest.iter().any(|word| sets_short_flag(word, 'x', "t"))
                || any_option_abbreviates(rest, &["extcmd"]) =>
        {
            Some(INTERPRETER)
        }
        "filter-branch" => Some(INTERPRETER),
        _ => None,
    }
}

/// Whether a short option cluster sets `flag` before a letter in `valued` takes the rest of
/// the word as its value (`-nO` sets `-O`; `-eFOO` is a pattern).
fn sets_short_flag(word: &str, flag: char, valued: &str) -> bool {
    if !is_short_cluster(word) {
        return false;
    }
    for letter in word[1..].chars() {
        if letter == flag {
            return true;
        }
        if valued.contains(letter) {
            return false;
        }
    }
    false
}

/// Only reads of `git config` run unasked: any change can name a command git
/// runs (pager, editor, hooks path, ssh command).
pub(super) fn git_config_risk(rest: &[String]) -> Option<&'static str> {
    const READ_ACTIONS: &[&str] = &[
        "--get",
        "--get-all",
        "--get-regexp",
        "--get-urlmatch",
        "--get-color",
        "--get-colorbool",
        "--list",
        "-l",
    ];
    const WRITE_ACTIONS: &[&str] = &[
        "--add",
        "--replace-all",
        "--unset",
        "--unset-all",
        "--rename-section",
        "--remove-section",
        "--edit",
        "-e",
    ];
    const VALUED: &[&str] = &[
        "-f",
        "--file",
        "--blob",
        "--type",
        "--default",
        "--comment",
        "--value",
    ];
    let has = |flags: &[&str]| rest.iter().any(|word| flags.contains(&word.as_str()));
    let first = positionals(rest, VALUED).first().copied();
    let reads = (has(READ_ACTIONS) || matches!(first, Some("get" | "list")))
        && !has(WRITE_ACTIONS)
        && !matches!(
            first,
            Some("set" | "unset" | "rename-section" | "remove-section" | "edit")
        );
    if reads {
        None
    } else if has(&["--global", "--system"]) {
        Some("risky:system")
    } else {
        Some(AGENT_CONFIG)
    }
}

fn hub_risk(
    args: &[String],
    reads: &[(&str, &[&str])],
    reason: &'static str,
) -> Option<&'static str> {
    let words = positionals(args, &["-R", "--repo"]);
    let group = *words.first()?;
    // `--web` hands the page to the browser, like `gh browse`.
    if args.iter().any(|word| word == "--web" || word == "-w") {
        return Some(reason);
    }
    if group == "api" {
        return api_mutates(args).then_some(reason);
    }
    let action = words.get(1).copied();
    let allowed = reads.iter().any(|(name, actions)| {
        *name == group
            && (actions.contains(&"*") || action.is_some_and(|action| actions.contains(&action)))
    });
    (!allowed).then_some(reason)
}

/// `gh api` and `glab api` send a request body, and so default to POST, when
/// given fields or input; an explicit method decides otherwise.
fn api_mutates(args: &[String]) -> bool {
    let mut method = None;
    let mut fields = false;
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        match word {
            "-X" | "--method" => method = Some(words.next().unwrap_or_default()),
            "-f" | "-F" | "--field" | "--raw-field" | "--input" => {
                fields = true;
                words.next();
            }
            _ if word.starts_with("--method=") => method = word.strip_prefix("--method="),
            _ if word.starts_with("-X") => method = word.strip_prefix("-X"),
            _ if word.starts_with("--field=")
                || word.starts_with("--raw-field=")
                || word.starts_with("--input=")
                || word.starts_with("-f")
                || word.starts_with("-F") =>
            {
                fields = true;
            }
            _ => {}
        }
    }
    method.map_or(fields, |method| !method.eq_ignore_ascii_case("GET"))
}

#[derive(Default)]
pub(super) struct Transfer<'a> {
    upload: bool,
    pub(super) outputs: Vec<&'a str>,
}

/// Short curl options that take a value; the rest of a cluster is that value.
const CURL_VALUED_SHORT: &str = "AbcCdDeEFHKmoPQrtTuUwxXyYz";

pub(super) fn curl_transfer(args: &[String]) -> Transfer<'_> {
    let mut transfer = Transfer::default();
    let mut index = 0;
    while let Some(word) = args.get(index) {
        index += 1;
        if word.starts_with("--") {
            let (name, inline) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(name, value)| (name, Some(value)));
            match name {
                "--request" => {
                    let method = inline.or_else(|| args.get(index).map(String::as_str));
                    index += usize::from(inline.is_none());
                    transfer.upload |=
                        method.is_none_or(|method| !method.eq_ignore_ascii_case("GET"));
                }
                "--output" | "--output-dir" | "--dump-header" | "--cookie-jar" | "--trace"
                | "--trace-ascii" | "--stderr" | "--libcurl" | "--etag-save" | "--hsts"
                | "--alt-svc" => {
                    let value = inline.or_else(|| args.get(index).map(String::as_str));
                    index += usize::from(inline.is_none());
                    transfer.outputs.extend(value);
                }
                _ if name.starts_with("--data")
                    || name.starts_with("--form")
                    || name.starts_with("--upload")
                    || name.starts_with("--mail")
                    || name == "--json"
                    || name == "--config" =>
                {
                    transfer.upload = true;
                }
                _ => {}
            }
        } else if is_short_cluster(word) {
            for (offset, letter) in word.char_indices().skip(1) {
                if !CURL_VALUED_SHORT.contains(letter) {
                    continue;
                }
                let attached = &word[offset + letter.len_utf8()..];
                let value = if attached.is_empty() {
                    index += 1;
                    args.get(index - 1).map(String::as_str)
                } else {
                    Some(attached)
                };
                match letter {
                    'X' => {
                        transfer.upload |=
                            value.is_none_or(|method| !method.eq_ignore_ascii_case("GET"));
                    }
                    'o' | 'D' | 'c' => transfer.outputs.extend(value),
                    'd' | 'F' | 'T' | 'K' => transfer.upload = true,
                    _ => {}
                }
                break;
            }
        } else if let Some((scheme, _)) = word.split_once("://") {
            transfer.upload |=
                !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"));
        }
    }
    transfer
}

/// Short wget options that take a value.
const WGET_VALUED_SHORT: &str = "aABDeiIlOoPQRtTUwX";

pub(super) fn wget_transfer(args: &[String]) -> Transfer<'_> {
    let mut transfer = Transfer::default();
    let mut index = 0;
    while let Some(word) = args.get(index) {
        index += 1;
        if word.starts_with("--") {
            let (name, inline) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(name, value)| (name, Some(value)));
            let mut value = || {
                let value = inline.or_else(|| args.get(index).map(String::as_str));
                index += usize::from(inline.is_none());
                value
            };
            match name {
                "--method" => {
                    transfer.upload |=
                        value().is_none_or(|method| !method.eq_ignore_ascii_case("GET"));
                }
                "--post-data" | "--post-file" | "--body-data" | "--body-file" | "--execute"
                | "--config" => {
                    transfer.upload = true;
                }
                "--output-document" | "--directory-prefix" | "--output-file"
                | "--append-output" => {
                    transfer.outputs.extend(value());
                }
                _ => {}
            }
        } else if is_short_cluster(word) {
            for (offset, letter) in word.char_indices().skip(1) {
                if !WGET_VALUED_SHORT.contains(letter) {
                    continue;
                }
                let attached = &word[offset + letter.len_utf8()..];
                let value = if attached.is_empty() {
                    index += 1;
                    args.get(index - 1).map(String::as_str)
                } else {
                    Some(attached)
                };
                match letter {
                    'e' => transfer.upload = true,
                    'O' | 'o' | 'a' | 'P' => transfer.outputs.extend(value),
                    _ => {}
                }
                break;
            }
        }
    }
    transfer
}

fn package_manager_risk(program: &str, args: &[String]) -> Option<&'static str> {
    let words = positionals(
        args,
        &[
            "-w",
            "--workspace",
            "--prefix",
            "-C",
            "--dir",
            "--filter",
            "-F",
            "--cwd",
        ],
    );
    // Publishing verbs are rare enough as ordinary words to match anywhere,
    // which also covers string options the skip list does not know.
    if words
        .iter()
        .any(|word| PACKAGE_PUBLISH_VERBS.contains(word))
    {
        return Some("risky:publish");
    }
    let first = words.first().copied();
    if words.iter().any(|word| matches!(*word, "exec" | "dlx")) || first == Some("x") {
        return Some(INTERPRETER);
    }
    // User configuration holds registry tokens.
    let changes_config = first == Some("set")
        || (first == Some("config")
            && words
                .iter()
                .any(|word| matches!(*word, "set" | "delete" | "edit" | "rm")));
    if changes_config {
        return Some(OUTSIDE);
    }
    let global = args
        .iter()
        .any(|word| matches!(word.as_str(), "-g" | "--global" | "--location=global"));
    let installs = words
        .iter()
        .any(|word| PACKAGE_INSTALL_VERBS.contains(word));
    let bare_link = words.len() == 1 && matches!(first, Some("link" | "ln"));
    ((global && installs) || bare_link || (program == "yarn" && first == Some("global")))
        .then_some("risky:install")
}

fn publisher_risk(program: &str, args: &[String]) -> Option<&'static str> {
    let words = positionals(args, &["-C", "--config", "-Z", "--color"]);
    let first = words.iter().copied().find(|word| !word.starts_with('+'));
    let any = |verbs: &[&str]| words.iter().any(|word| verbs.contains(word));
    match program {
        "cargo" if any(&["publish", "yank", "owner", "login", "logout"]) => Some("risky:publish"),
        "cargo" if first == Some("binstall") => Some("risky:install"),
        "cargo" | "gem" | "go" if first == Some("install") => Some("risky:install"),
        "gem" if any(&["push", "yank", "owner", "signin"]) => Some("risky:publish"),
        "twine" if any(&["upload", "register"]) => Some("risky:publish"),
        "buildah" if any(&["push"]) => Some("risky:publish"),
        "skopeo" if any(&["copy", "sync"]) => Some("risky:publish"),
        "oras" if any(&["push", "cp", "copy"]) => Some("risky:publish"),
        "crane" if any(&["push", "copy", "cp"]) => Some("risky:publish"),
        tool if CONTAINER_TOOLS.contains(&tool)
            && (any(&["push", "login"])
                || args.iter().any(|word| word == "--push")
                || registry_output(args)) =>
        {
            Some("risky:publish")
        }
        tool if CONTAINER_TOOLS.contains(&tool)
            && args.iter().any(|word| {
                word == "--privileged"
                    || word == "--pid=host"
                    || word.starts_with("--cap-add")
                    || word.starts_with("--device")
            }) =>
        {
            Some("risky:system")
        }
        _ => None,
    }
}

/// An `--output` / `-o` value (buildx) that writes the image to a `registry`.
fn registry_output(args: &[String]) -> bool {
    let mut values: Vec<&str> = Vec::new();
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "--output" || word == "-o" {
            values.extend(words.next());
        } else if let Some(value) = word
            .strip_prefix("--output=")
            .or_else(|| word.strip_prefix("-o"))
        {
            if !value.is_empty() {
                values.push(value);
            }
        }
    }
    values
        .iter()
        .any(|value| value.contains("registry") || value.contains("push=true"))
}

/// Publish or deploy verbs for language and build tools not in the package
/// managers above.
fn publish_tool_risk(program: &str, args: &[String]) -> Option<&'static str> {
    let words = positionals(args, &[]);
    let first = words.first().copied();
    let has = |verb: &str| words.contains(&verb);
    match program {
        "poetry" | "pdm" | "hatch" | "flit" | "uv" if first == Some("publish") => {
            Some("risky:publish")
        }
        "mvn" if has("deploy") => Some("risky:publish"),
        "gradle" | "gradlew" if words.iter().any(|word| word.contains("publish")) => {
            Some("risky:publish")
        }
        "dotnet" if first == Some("nuget") && has("push") => Some("risky:publish"),
        _ => None,
    }
}

/// awk programs that run a command: `system(`, a `|&` coprocess, an output
/// pipe (`print … |`), or an input pipe (`"cmd" | getline`). A program read
/// from a file (`-f`) is not visible, so it counts too.
fn awk_executes(args: &[String]) -> bool {
    args.iter().any(|word| {
        word == "-f"
            || word.starts_with("--file")
            || word.contains("system(")
            || word.contains("|&")
            || (word.contains("print") && word.contains('|'))
            || pipes_into_getline(word)
    })
}

/// `… | getline`: a `|` followed, after optional blanks, by `getline`.
fn pipes_into_getline(program: &str) -> bool {
    program.match_indices('|').any(|(at, _)| {
        program[at + 1..]
            .trim_start_matches([' ', '\t'])
            .starts_with("getline")
    })
}

/// GNU sed scripts that shell out: an `e` command (after any address) or an
/// `s///e` flag. A script read from a file (`-f`) is not visible, so it counts.
fn sed_executes(args: &[String]) -> bool {
    let mut scripts: Vec<&str> = Vec::new();
    let mut positional: Vec<&str> = Vec::new();
    let mut explicit = false;
    // BSD `sed -i ''` / `sed -i .bak` take the backup suffix as its own word.
    let mut suffix_word = false;
    let mut words = args.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if matches!(word, "-f" | "--file") || word.starts_with("--file=") {
            return true;
        }
        if matches!(word, "-e" | "--expression") {
            explicit = true;
            scripts.extend(words.next());
        } else if let Some(value) = word.strip_prefix("--expression=") {
            explicit = true;
            scripts.push(value);
        } else if word.starts_with("--") {
            // `--in-place[=suffix]`, `--quiet`, `--regexp-extended`, …
        } else if let Some(cluster) = word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            // Short options: `i` takes an attached suffix; `e`, `f` and `l`
            // take the rest of the cluster or the next word.
            for (offset, letter) in cluster.char_indices() {
                let rest = &cluster[offset + 1..];
                match letter {
                    'i' => {
                        suffix_word = rest.is_empty();
                        break;
                    }
                    'f' => return true,
                    'e' | 'l' => {
                        let value = if rest.is_empty() {
                            words.next()
                        } else {
                            Some(rest)
                        };
                        if letter == 'e' {
                            explicit = true;
                            scripts.extend(value);
                        }
                        break;
                    }
                    _ => {}
                }
            }
        } else if std::mem::take(&mut suffix_word) && (word.is_empty() || word.starts_with('.')) {
            // The suffix, not the script.
        } else {
            positional.push(word);
        }
    }
    if !explicit {
        scripts.extend(positional.first());
    }
    scripts.iter().any(|script| script_shells_out(script))
}

/// Walks sed commands: optional address (`N`, `$`, `/re/`, `\cREc`, with an
/// optional `,addr`, `~N` or `+N`) and `!`, then the command letter.
fn script_shells_out(script: &str) -> bool {
    let chars: Vec<char> = script.chars().collect();
    let mut index = 0;
    let skip_blank = |index: &mut usize| {
        while chars
            .get(*index)
            .is_some_and(|c| matches!(c, ' ' | '\t' | ';' | '\n'))
        {
            *index += 1;
        }
    };
    while index < chars.len() {
        skip_blank(&mut index);
        index = skip_address(&chars, index);
        if chars.get(index) == Some(&',') {
            index = skip_address(&chars, index + 1);
        }
        while chars
            .get(index)
            .is_some_and(|c| matches!(c, ' ' | '\t' | '!'))
        {
            index += 1;
        }
        let Some(&command) = chars.get(index) else {
            break;
        };
        index += 1;
        match command {
            'e' => return true,
            '{' | '}' => {}
            's' => {
                let Some(end) = skip_delimited(&chars, index, 2) else {
                    return false;
                };
                // Flags run to the command end; `w file` consumes the rest.
                index = end;
                while let Some(&c) = chars.get(index) {
                    if matches!(c, ';' | '\n' | '}' | 'w') {
                        break;
                    }
                    if c == 'e' {
                        return true;
                    }
                    index += 1;
                }
            }
            'y' => {
                index = skip_delimited(&chars, index, 2).unwrap_or(chars.len());
            }
            // Text and file arguments run to the end of the line.
            'a' | 'i' | 'c' | 'r' | 'R' | 'w' | 'W' | ':' | 'b' | 't' | 'T' => {
                while chars.get(index).is_some_and(|c| *c != '\n') {
                    index += 1;
                }
            }
            _ => {
                while chars
                    .get(index)
                    .is_some_and(|c| !matches!(c, ';' | '\n' | '}'))
                {
                    index += 1;
                }
            }
        }
    }
    false
}

/// Index after one sed address at `index`, or `index` when there is none.
fn skip_address(chars: &[char], mut index: usize) -> usize {
    match chars.get(index) {
        Some(c) if c.is_ascii_digit() => {
            while chars.get(index).is_some_and(char::is_ascii_digit) {
                index += 1;
            }
        }
        Some('$') => index += 1,
        Some('/') => return skip_delimited(chars, index, 1).unwrap_or(chars.len()),
        Some('\\') if index + 1 < chars.len() => {
            return skip_delimited(chars, index + 1, 1).unwrap_or(chars.len());
        }
        Some('+' | '~') => {
            index += 1;
            while chars.get(index).is_some_and(char::is_ascii_digit) {
                index += 1;
            }
            return index;
        }
        _ => return index,
    }
    // `first~step` on the first address.
    if chars.get(index) == Some(&'~') {
        index += 1;
        while chars.get(index).is_some_and(char::is_ascii_digit) {
            index += 1;
        }
    }
    // Regex address flags (`I`, `M`).
    while chars.get(index).is_some_and(|c| matches!(c, 'I' | 'M')) {
        index += 1;
    }
    index
}

/// With the delimiter at `index`, the index after `parts` more unescaped
/// delimiters (so `parts = 2` skips an `s` or `y` body). `None` if unterminated.
fn skip_delimited(chars: &[char], index: usize, parts: usize) -> Option<usize> {
    let delimiter = *chars.get(index)?;
    let mut cursor = index + 1;
    let mut seen = 0;
    while cursor < chars.len() {
        match chars[cursor] {
            '\\' => cursor += 1,
            c if c == delimiter => {
                seen += 1;
                if seen == parts {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

/// Deploy and infrastructure CLIs matched by their deploying verbs (single or
/// two-word). `ansible-playbook` always deploys.
const DEPLOY_VERB_TOOLS: &[(&[&str], &[&str], bool)] = &[
    (
        &["wrangler"],
        &["deploy", "publish", "pages deploy", "delete", "rollback"],
        false,
    ),
    (&["serverless", "sls"], &["deploy", "remove"], false),
    (&["cdk"], &["deploy", "destroy", "import"], false),
    (&["sam"], &["deploy", "sync", "publish"], false),
    (
        &["kamal"],
        &["deploy", "redeploy", "rollback", "setup", "remove"],
        false,
    ),
    (&["helmfile"], &["apply", "sync", "destroy"], false),
    (&["kapp"], &["deploy", "delete"], false),
    (
        &["argocd"],
        &[
            "app sync",
            "app create",
            "app delete",
            "app set",
            "app rollback",
        ],
        false,
    ),
    (
        &["flux"],
        &["reconcile", "bootstrap", "install", "delete"],
        false,
    ),
    (
        &["supabase"],
        &["db push", "db reset", "functions deploy", "link"],
        false,
    ),
    (&["railway"], &["up", "deploy", "redeploy", "down"], false),
    (&["eb"], &["deploy", "create", "terminate"], false),
    (&["amplify"], &["push", "publish", "delete"], false),
    (
        &["oc"],
        &[
            "apply", "delete", "create", "patch", "replace", "scale", "rollout", "expose", "edit",
            "new-app",
        ],
        false,
    ),
    (&["ansible-playbook"], &[], true),
];

fn deploy_verb_risk(program: &str, args: &[String]) -> Option<&'static str> {
    let (_, verbs, always) = DEPLOY_VERB_TOOLS
        .iter()
        .find(|(names, _, _)| names.contains(&program))?;
    if *always {
        return Some("risky:deploy");
    }
    let positional = positionals(args, &[]);
    let one = positional.first().copied();
    let two = match (positional.first(), positional.get(1)) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        _ => None,
    };
    let matched = verbs
        .iter()
        .any(|verb| Some(*verb) == one || two.as_deref() == Some(*verb));
    matched.then_some("risky:deploy")
}

fn is_deploy_verb_tool(program: &str) -> bool {
    DEPLOY_VERB_TOOLS
        .iter()
        .any(|(names, _, _)| names.contains(&program))
}

/// Toolchain managers that install into the user's global environment.
fn toolchain_risk(program: &str, args: &[String]) -> Option<&'static str> {
    let words = positionals(args, &[]);
    let first = words.first().copied();
    let second = words.get(1).copied();
    let flagged = |flags: &[&str]| args.iter().any(|word| flags.contains(&word.as_str()));
    let installs = match program {
        "pip" | "pip3" => {
            first == Some("install") && flagged(&["--user", "--break-system-packages", "--system"])
        }
        "pipx" => first.is_some_and(|first| {
            ![
                "list",
                "run",
                "environment",
                "completions",
                "--version",
                "help",
            ]
            .contains(&first)
        }),
        "uv" => {
            (first == Some("tool") && matches!(second, Some("install" | "upgrade" | "uninstall")))
                || (first == Some("pip") && flagged(&["--system", "--break-system-packages"]))
                || (first == Some("python") && matches!(second, Some("install" | "uninstall")))
        }
        _ => first.is_some_and(|first| {
            !["show", "which", "doc", "help", "--version", "-V"].contains(&first)
        }),
    };
    installs.then_some("risky:install")
}

/// `host:path`, `user@host:path`, `host::module` or `rsync://`, or a remote shell.
fn rsync_is_remote(args: &[String]) -> bool {
    let remote_shell = options(args).any(|option| {
        option.starts_with("--rsh") || (is_short_cluster(option) && option.contains('e'))
    });
    remote_shell
        || operands(args).iter().any(|operand| {
            operand.starts_with("rsync://")
                || operand
                    .find(':')
                    .is_some_and(|colon| !operand[..colon].contains('/'))
        })
}
