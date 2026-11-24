use std::{collections::HashSet, fmt, path::PathBuf, str::FromStr};

pub const CLI_USAGE: &str = "Usage: iface [options] [command]

Commands:
  exec [prompt]             Run one non-interactive prompt (or - for stdin)
  doctor [providers]        Print configured inventory (not startup health);
                            providers checks each CLI against what Interface
                            depends on (help and version only; no model turns)
  ps                        List background exec children
  logs <name>               Print a background child's log
  kill <name>               Stop a background child
  agent <verb>              Control named sessions in the running TUI
  orchestrate <verb>        Control shared multi-agent runs
  away <verb>               Run or inspect an Away pilot or reviewed project cycle
  workers <verb>            Serve, connect or launch optional cheap workers
  mcp                       Serve Interface's threads to a Claude Code or Codex
                            outside Interface (MCP over stdio; see iface mcp --help)
  theme convert <file>      Print a theme file converted from another app's scheme
  trigger <name or id>      Run a thread's scheduled prompt now (Interface must be open)

Options:
  --version                 Print the package version and exit
  --help                    Show this help and exit
  --continue                Reopen the most recent session for --cwd
  --resume <id-prefix>      Resume the one session matching this id prefix
  --tui <inline|fullscreen> Select the terminal rendering mode
  --cwd <dir>               Start or continue in this directory
  --engine <id>             Select claude, codex, or an acp:<slug> engine
  --scope <all|project|isolated>
                            Which on-disk settings the session loads
  --output <text|json|jsonl>
                            Non-interactive output format (exec/doctor/ps)
  --timeout <duration>      Stop waiting for exec after this (ms, 30s, 5m)
  --bg <name>               Run exec as a background child";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliCommand {
    Exec,
    Doctor,
    Ps,
    Logs,
    Kill,
    Agent,
    Orchestrate,
    Away,
    Workers,
    Theme,
    Trigger,
    Mcp,
}

impl FromStr for CliCommand {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "exec" => Ok(Self::Exec),
            "doctor" => Ok(Self::Doctor),
            "ps" => Ok(Self::Ps),
            "logs" => Ok(Self::Logs),
            "kill" => Ok(Self::Kill),
            "agent" => Ok(Self::Agent),
            "orchestrate" => Ok(Self::Orchestrate),
            "away" => Ok(Self::Away),
            "workers" => Ok(Self::Workers),
            "theme" => Ok(Self::Theme),
            "trigger" => Ok(Self::Trigger),
            "mcp" => Ok(Self::Mcp),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiMode {
    Inline,
    Fullscreen,
}

impl TuiMode {
    /// Returns the exact flag and config spelling of this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Fullscreen => "fullscreen",
        }
    }

    /// The mode spelled exactly as [`Self::as_str`] writes it.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "inline" => Some(Self::Inline),
            "fullscreen" => Some(Self::Fullscreen),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchScope {
    All,
    Project,
    Isolated,
}

impl LaunchScope {
    /// Returns the exact wire and config spelling of this scope.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Project => "project",
            Self::Isolated => "isolated",
        }
    }

    /// The scope spelled exactly as [`Self::as_str`] writes it.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "project" => Some(Self::Project),
            "isolated" => Some(Self::Isolated),
            _ => None,
        }
    }

    /// Returns the exact Claude setting-source value for this launch.
    #[must_use]
    pub fn setting_sources(self) -> Option<Vec<&'static str>> {
        match self {
            Self::All => None,
            Self::Project => Some(vec!["project"]),
            Self::Isolated => Some(Vec::new()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputFormat {
    Text,
    Json,
    JsonLines,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliArgv {
    pub version: bool,
    pub help: bool,
    pub continue_session: bool,
    pub resume: Option<String>,
    pub tui: Option<TuiMode>,
    pub cwd: Option<PathBuf>,
    pub engine: Option<String>,
    pub scope: Option<LaunchScope>,
    pub command: Option<CliCommand>,
    pub output: Option<OutputFormat>,
    pub timeout_ms: Option<u64>,
    pub background_name: Option<String>,
    pub prompt: Option<String>,
    pub name: Option<String>,
    pub agent_args: Vec<String>,
    pub orchestration_args: Vec<String>,
    pub away_args: Vec<String>,
    pub worker_args: Vec<String>,
    pub theme_args: Vec<String>,
    /// `iface mcp`'s own arguments (T3 section 11).
    pub mcp_args: Vec<String>,
    /// What `doctor` reports.
    pub doctor_report: DoctorReport,
}

/// `doctor` alone, or `doctor providers`: the provider CLI drift canary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DoctorReport {
    #[default]
    Inventory,
    Providers,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgvError(String);

impl ArgvError {
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        2
    }
}

impl fmt::Display for ArgvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgvError {}

/// Parses the stable `iface` argument grammar without interpreting delegated
/// agent or orchestration arguments.
///
/// # Errors
///
/// Returns a validation error for unknown, duplicate, incomplete, or
/// command-incompatible arguments.
pub fn parse_argv(args: &[String]) -> Result<CliArgv, ArgvError> {
    let mut result = CliArgv::default();
    let mut seen = HashSet::new();
    let mut positionals = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let flag = &args[index];
        if matches!(
            flag.as_str(),
            "agent" | "orchestrate" | "away" | "workers" | "theme" | "mcp"
        ) && positionals.is_empty()
        {
            positionals.extend_from_slice(&args[index..]);
            break;
        }
        if flag == "--" {
            positionals.extend_from_slice(&args[index + 1..]);
            break;
        }
        if flag == "-" || !flag.starts_with('-') {
            positionals.push(flag.clone());
            index += 1;
            continue;
        }
        if !seen.insert(flag.clone()) {
            return Err(ArgvError(format!("{flag} may only be specified once")));
        }
        match flag.as_str() {
            "--version" => result.version = true,
            "--help" => result.help = true,
            "--continue" => result.continue_session = true,
            "--resume" => {
                result.resume = Some(require_value(args, &mut index, flag)?);
            }
            "--tui" => {
                result.tui = Some(
                    TuiMode::parse(&require_value(args, &mut index, flag)?)
                        .ok_or_else(|| ArgvError("--tui must be inline or fullscreen".into()))?,
                );
            }
            "--cwd" => {
                let cwd = PathBuf::from(require_value(args, &mut index, flag)?);
                result.cwd =
                    Some(std::path::absolute(cwd).map_err(|error| {
                        ArgvError(format!("--cwd could not be resolved: {error}"))
                    })?);
            }
            "--engine" => {
                let engine = require_value(args, &mut index, flag)?;
                if !is_engine_id(&engine) {
                    return Err(ArgvError(
                        "--engine must be claude, codex, or acp:<slug>".into(),
                    ));
                }
                result.engine = Some(engine);
            }
            "--scope" => {
                result.scope = Some(
                    LaunchScope::parse(&require_value(args, &mut index, flag)?).ok_or_else(
                        || ArgvError("--scope must be all, project, or isolated".into()),
                    )?,
                );
            }
            "--output" => {
                result.output = Some(match require_value(args, &mut index, flag)?.as_str() {
                    "text" => OutputFormat::Text,
                    "json" => OutputFormat::Json,
                    "jsonl" => OutputFormat::JsonLines,
                    _ => {
                        return Err(ArgvError("--output must be text, json, or jsonl".into()));
                    }
                });
            }
            "--timeout" => {
                result.timeout_ms =
                    Some(parse_duration_ms(&require_value(args, &mut index, flag)?)?);
            }
            "--bg" => result.background_name = Some(require_value(args, &mut index, flag)?),
            _ => return Err(ArgvError(format!("Unknown option: {flag}"))),
        }
        index += 1;
    }

    parse_positionals(&mut result, &positionals)?;
    validate_combinations(&result)?;
    Ok(result)
}

fn require_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, ArgvError> {
    let next = index.saturating_add(1);
    let value = args
        .get(next)
        .filter(|value| !value.starts_with("--") && !value.is_empty())
        .ok_or_else(|| ArgvError(format!("{flag} requires a value")))?;
    *index = next;
    Ok(value.clone())
}

fn parse_positionals(result: &mut CliArgv, positionals: &[String]) -> Result<(), ArgvError> {
    let Some(head) = positionals.first() else {
        return Ok(());
    };
    let command = CliCommand::from_str(head)
        .map_err(|()| ArgvError(format!("Unexpected argument: {head}")))?;
    result.command = Some(command);
    let rest = &positionals[1..];
    match command {
        CliCommand::Agent => result.agent_args.extend_from_slice(rest),
        CliCommand::Away => result.away_args.extend_from_slice(rest),
        CliCommand::Workers => result.worker_args.extend_from_slice(rest),
        CliCommand::Theme => result.theme_args.extend_from_slice(rest),
        CliCommand::Mcp => result.mcp_args.extend_from_slice(rest),
        CliCommand::Orchestrate => {
            result.orchestration_args.extend_from_slice(rest);
        }
        CliCommand::Trigger => {
            let Some(name) = rest.first().filter(|name| !name.trim().is_empty()) else {
                return Err(ArgvError(
                    "trigger requires a scheduled prompt's name or id".into(),
                ));
            };
            result.name = Some(name.clone());
            if let Some(extra) = rest.get(1) {
                return Err(ArgvError(format!("Unexpected argument: {extra}")));
            }
        }
        CliCommand::Logs | CliCommand::Kill => {
            let Some(name) = rest.first() else {
                return Err(ArgvError(format!("{head} requires a session name")));
            };
            result.name = Some(name.clone());
            if let Some(extra) = rest.get(1) {
                return Err(ArgvError(format!("Unexpected argument: {extra}")));
            }
        }
        CliCommand::Doctor => match rest {
            [] => {}
            [only] if only == "providers" => result.doctor_report = DoctorReport::Providers,
            [only] => return Err(ArgvError(format!("Unexpected argument: {only}"))),
            [_, extra, ..] => return Err(ArgvError(format!("Unexpected argument: {extra}"))),
        },
        CliCommand::Ps => {
            if let Some(extra) = rest.first() {
                return Err(ArgvError(format!("Unexpected argument: {extra}")));
            }
        }
        CliCommand::Exec => {
            if !rest.is_empty() {
                result.prompt = Some(rest.join(" "));
            }
        }
    }
    Ok(())
}

fn validate_combinations(result: &CliArgv) -> Result<(), ArgvError> {
    if result.continue_session && result.resume.is_some() {
        return Err(ArgvError(
            "--continue and --resume cannot be used together".into(),
        ));
    }
    if result.background_name.is_some() && result.command != Some(CliCommand::Exec) {
        return Err(ArgvError("--bg is only valid with exec".into()));
    }
    if result.output.is_some()
        && !matches!(
            result.command,
            Some(CliCommand::Exec | CliCommand::Doctor | CliCommand::Ps)
        )
    {
        return Err(ArgvError(
            "--output is only valid with exec, doctor, or ps".into(),
        ));
    }
    if result.command == Some(CliCommand::Doctor) && result.output == Some(OutputFormat::JsonLines)
    {
        return Err(ArgvError("doctor supports --output text or json".into()));
    }
    if result.timeout_ms.is_some() && result.command != Some(CliCommand::Exec) {
        return Err(ArgvError("--timeout is only valid with exec".into()));
    }
    if result.scope.is_some()
        && result.command.is_some()
        && result.command != Some(CliCommand::Exec)
    {
        return Err(ArgvError(
            "--scope is only valid with exec or the interactive TUI".into(),
        ));
    }
    Ok(())
}

/// Accepts the exact engine ids the CLI, config, agent client and steering
/// ledger all admit. This is deliberately stricter than `EngineId::new`, which
/// accepts any non-empty `acp:` suffix.
#[must_use]
pub(crate) fn is_engine_id(value: &str) -> bool {
    if matches!(value, "claude" | "codex") {
        return true;
    }
    let Some(slug) = value.strip_prefix("acp:") else {
        return false;
    };
    let bytes = slug.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Converts a CLI duration to milliseconds.
///
/// # Errors
///
/// Returns a validation error for a malformed or overflowing duration.
pub fn parse_duration_ms(value: &str) -> Result<u64, ArgvError> {
    let (digits, factor) = if let Some(value) = value.strip_suffix("ms") {
        (value, 1_u64)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1_000)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60_000)
    } else if let Some(value) = value.strip_suffix('h') {
        (value, 3_600_000)
    } else {
        (value, 1)
    };
    let amount = digits
        .parse::<u64>()
        .map_err(|_| ArgvError("--timeout must be a duration such as 5000, 30s, or 5m".into()))?;
    amount
        .checked_mul(factor)
        .ok_or_else(|| ArgvError("--timeout must be a duration such as 5000, 30s, or 5m".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Result<CliArgv, ArgvError> {
        parse_argv(
            &values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn parses_the_complete_launch_surface() {
        let parsed = parse(&[
            "--tui",
            "fullscreen",
            "--cwd",
            "project",
            "--engine",
            "acp:qwen",
            "--resume",
            "abc",
            "--version",
            "--help",
        ])
        .expect("valid arguments");
        assert!(parsed.version);
        assert!(parsed.help);
        assert_eq!(parsed.tui, Some(TuiMode::Fullscreen));
        assert_eq!(parsed.engine.as_deref(), Some("acp:qwen"));
        assert_eq!(parsed.resume.as_deref(), Some("abc"));
        assert!(parsed.cwd.expect("cwd").is_absolute());
    }

    /// `iface trigger <name or id>` (T3 section 7) takes exactly one name.
    #[test]
    fn trigger_takes_one_schedule_name_or_id() {
        let parsed = parse(&["trigger", "Nightly check"]).expect("trigger");
        assert_eq!(parsed.command, Some(CliCommand::Trigger));
        assert_eq!(parsed.name.as_deref(), Some("Nightly check"));
        assert!(parse(&["trigger"]).is_err());
        assert!(parse(&["trigger", "a", "b"]).is_err());
        assert!(parse(&["trigger", "a", "--output", "json"]).is_err());
        assert!(CLI_USAGE.contains("trigger <name or id>"));
    }

    #[test]
    fn parses_exec_and_delegated_commands_without_reinterpreting_them() {
        let exec =
            parse(&["exec", "--output", "json", "--timeout", "30s", "do", "it"]).expect("exec");
        assert_eq!(exec.command, Some(CliCommand::Exec));
        assert_eq!(exec.output, Some(OutputFormat::Json));
        assert_eq!(exec.timeout_ms, Some(30_000));
        assert_eq!(exec.prompt.as_deref(), Some("do it"));

        let agent = parse(&[
            "agent",
            "wait",
            "reviewer",
            "--status",
            "idle",
            "--timeout",
            "30s",
        ])
        .expect("delegated agent command");
        assert_eq!(
            agent.agent_args,
            ["wait", "reviewer", "--status", "idle", "--timeout", "30s"]
        );
    }

    #[test]
    fn scope_preserves_omitted_project_and_isolated_distinctions() {
        assert_eq!(
            LaunchScope::All.setting_sources(),
            None,
            "all means omit the provider option"
        );
        assert_eq!(
            LaunchScope::Project.setting_sources(),
            Some(vec!["project"])
        );
        assert_eq!(LaunchScope::Isolated.setting_sources(), Some(Vec::new()));
    }

    #[test]
    fn rejects_legacy_invalid_combinations() {
        for (args, expected) in [
            (vec!["--wat"], "Unknown option"),
            (
                vec!["--continue", "--resume", "a"],
                "cannot be used together",
            ),
            (vec!["exec", "--output", "xml"], "text, json, or jsonl"),
            (vec!["logs"], "requires a session name"),
            (vec!["--bg", "job"], "only valid with exec"),
            (vec!["doctor", "--output", "jsonl"], "text or json"),
            (
                vec!["doctor", "--scope", "all"],
                "only valid with exec or the interactive TUI",
            ),
        ] {
            let error = parse(&args).expect_err("invalid arguments");
            assert!(error.to_string().contains(expected), "{args:?}: {error}");
            assert_eq!(error.exit_code(), 2);
        }
    }

    #[test]
    fn doctor_providers_selects_the_provider_canary_and_nothing_else() {
        let parsed = parse(&["doctor", "providers", "--output", "json"]).expect("valid");
        assert_eq!(parsed.command, Some(CliCommand::Doctor));
        assert_eq!(parsed.doctor_report, DoctorReport::Providers);
        assert_eq!(
            parse(&["doctor"]).expect("valid").doctor_report,
            DoctorReport::Inventory
        );
        for args in [
            vec!["doctor", "everything"],
            vec!["doctor", "providers", "more"],
        ] {
            let error = parse(&args).expect_err("one optional word");
            assert!(
                error.to_string().contains("Unexpected argument"),
                "{args:?}"
            );
        }
    }

    /// `iface mcp` takes only its own arguments, which its module parses;
    /// every top-level option before it is still checked.
    #[test]
    fn mcp_delegates_its_arguments_and_parses_only_data_root() {
        let bare = parse(&["mcp"]).expect("iface mcp");
        assert_eq!(bare.command, Some(CliCommand::Mcp));
        assert!(bare.mcp_args.is_empty());
        let rooted = parse(&["mcp", "--data-root", "/tmp/iface-data"]).expect("with a root");
        assert_eq!(rooted.mcp_args, ["--data-root", "/tmp/iface-data"]);
        let options = crate::mcp::parse(&rooted.mcp_args).expect("its own grammar");
        assert_eq!(
            options.data_root.as_deref(),
            Some(std::path::Path::new("/tmp/iface-data"))
        );
        assert!(crate::mcp::parse(&["--help".into()]).unwrap().help);
        for (args, expected) in [
            (vec!["--data-root"], "requires a folder"),
            (
                vec!["--data-root", "/a", "--data-root", "/b"],
                "only be specified once",
            ),
            (vec!["--level", "full"], "Unexpected argument: --level"),
            (vec!["--client", "me"], "Unexpected argument: --client"),
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let error = crate::mcp::parse(&args).expect_err("refused");
            assert!(error.contains(expected), "{args:?}: {error}");
        }
        assert!(CLI_USAGE.contains("mcp"));
    }

    #[test]
    fn duration_parser_matches_the_legacy_units() {
        assert_eq!(parse_duration_ms("5000").expect("milliseconds"), 5_000);
        assert_eq!(parse_duration_ms("30s").expect("seconds"), 30_000);
        assert_eq!(parse_duration_ms("5m").expect("minutes"), 300_000);
        assert_eq!(parse_duration_ms("1h").expect("hours"), 3_600_000);
        assert!(parse_duration_ms("1.5m").is_err());
    }
}
