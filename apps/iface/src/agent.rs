/// The legacy `iface agent` help text plus `list --live` and the note on long
/// lists. Help is a local parser fast path and must never attempt to connect
/// to the long-lived TUI control socket.
pub const AGENT_USAGE: &str = "Usage:
  iface agent list [--live]
  iface agent commands [--output text|json]
  iface agent get <target>
  iface agent read <target> [--lines N]
  iface agent send <target> <text|-> [--sender <target>] [--message-id <id>]
                                    [--reply-to <message-id>] [--expect-reply]
  iface agent steer <target> <text|-> [--sender <target>] [--message-id <id>]
                                     [--reply-to <message-id>] [--expect-reply]
  iface agent stop <target>
  iface agent inbox <target> [--after <cursor>] [--limit <1-100>]
  iface agent monitor <target> [--after <cursor>] [--limit <1-100>]
                               [--timeout <duration>]
  iface agent wait <target> --status <idle|working|blocked> [--timeout <duration>]
  iface agent resume <target>
  iface agent start <name> [--engine <id>] [--cwd <dir>] [--model <id>] [--effort <level>]
                           [--scope <all|project|isolated>]
                           [--prompt <text|->|--brief <text|->]
                           [--plan-id <id>] [--task-id <id>] [--parent-session <key>]
                           [--role <role>] [--task-scope <scope>] [--brief-hash <digest>]
  iface agent rename <target> <name>|--clear
  iface agent focus <target>

Options:
  --output <text|json>       Select output format
  --live                     List only live sessions
  --help                     Show this help and exit

list shows live and named sessions first; stored sessions that do not fit in
one reply are left out.";

/// Returns whether the delegated grammar requests help without requiring a
/// live control host. The legacy parser accepts `--help` as the verb and on a
/// recognized verb.
#[must_use]
pub fn is_help_request(args: &[String]) -> bool {
    let Some(verb) = args.first().map(String::as_str) else {
        return false;
    };
    verb == "--help"
        || (crate::agent_protocol::AgentMethod::from_verb(verb).is_some()
            && args.iter().skip(1).any(|arg| arg == "--help"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_fast_path_matches_legacy_forms() {
        assert!(is_help_request(&["--help".into()]));
        assert!(is_help_request(&["list".into(), "--help".into()]));
        assert!(!is_help_request(&[]));
        assert!(!is_help_request(&["unknown".into(), "--help".into()]));
    }
}
