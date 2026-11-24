//! `OpenCode`'s TUI title is identity only. Project its private v1 state snapshots
//! in Rust; the embedded TUI transport only copies provider-owned fields.
use crate::osc_title::{FilteredOscChunk, ReservedOscFilter};

pub const OPENCODE_ACTIVITY_TRANSPORT: &str = include_str!("opencode_activity_transport.mjs");
pub const TERMINAL_ACTIVITY_PREFIX: &str = "interface-terminal-activity:v1:";
const SNAPSHOT_PREFIX: &str = "interface-opencode-state:v1:";
const MAX_SNAPSHOT_BYTES: usize = 8_192;

/// Keep the ordering of private activity and provider-identity filters in one
/// place, including flushing an incomplete unrelated escape at EOF.
pub(crate) struct TerminalOutputFilter {
    identity: ReservedOscFilter,
    activity: Option<OpenCodeActivityBridge>,
}

impl TerminalOutputFilter {
    pub(crate) fn new(opencode: bool) -> Self {
        Self {
            identity: ReservedOscFilter::new(crate::launch::CLAUDE_SESSION_BINDING_TITLE_PREFIX),
            activity: opencode.then(OpenCodeActivityBridge::new),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> FilteredOscChunk {
        let activity = self.activity.as_mut().map(|bridge| bridge.feed(bytes));
        self.identity.feed(activity.as_deref().unwrap_or(bytes))
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let mut trailing = Vec::new();
        if let Some(activity) = &mut self.activity {
            trailing.extend(self.identity.feed(&activity.finish()).visible);
        }
        trailing.extend(self.identity.finish());
        trailing
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Activity {
    Idle,
    Working,
    Waiting,
}

struct Session<'a> {
    id: &'a str,
    parent: &'a str,
    status: &'a str,
    pending: bool,
}

fn identifier(value: &str) -> bool {
    value == "-"
        || (value.starts_with("ses_")
            && value.len() <= 100
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
}

fn project(snapshot: &str) -> Option<(&str, Activity)> {
    let mut fields = snapshot.split('|');
    let selected = fields.next()?;
    if !identifier(selected) {
        return None;
    }
    let mut sessions = Vec::new();
    for field in fields {
        if sessions.len() >= 32 {
            return None;
        }
        let row: Vec<_> = field.split(',').collect();
        if row.len() != 5 || !identifier(row[0]) || !identifier(row[1]) {
            return None;
        }
        if !matches!(row[2], "idle" | "busy" | "retry") {
            return None;
        }
        let permissions: u32 = row[3].parse().ok()?;
        let questions: u32 = row[4].parse().ok()?;
        sessions.push(Session {
            id: row[0],
            parent: row[1],
            status: row[2],
            pending: permissions > 0 || questions > 0,
        });
    }
    if selected == "-" {
        return Some((selected, Activity::Idle));
    }
    let current = sessions.iter().find(|session| session.id == selected)?;
    // OpenCode displays a root session's own and direct children's approvals.
    // A child view does not display the parent's/siblings' permission prompts.
    let waiting = current.parent == "-"
        && sessions.iter().any(|session| {
            (session.id == selected || session.parent == selected) && session.pending
        });
    let activity = if waiting {
        Activity::Waiting
    } else if matches!(current.status, "busy" | "retry") {
        Activity::Working
    } else {
        Activity::Idle
    };
    Some((selected, activity))
}

pub(crate) struct OpenCodeActivityBridge {
    filter: ReservedOscFilter,
    previous: Option<(String, Activity)>,
}

impl OpenCodeActivityBridge {
    pub(crate) fn new() -> Self {
        Self {
            filter: ReservedOscFilter::with_limit(SNAPSHOT_PREFIX, MAX_SNAPSHOT_BYTES),
            previous: None,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        let filtered = self.filter.feed(bytes);
        let mut visible = filtered.visible;
        for snapshot in filtered.reserved_payloads {
            let Some((session, activity)) = project(&snapshot) else {
                continue;
            };
            let completed = self.previous.as_ref().is_some_and(|(previous, state)| {
                previous == session && *state != Activity::Idle && activity == Activity::Idle
            });
            let signal = match (activity, completed) {
                (Activity::Idle, true) => "complete",
                (Activity::Idle, false) => "idle",
                (Activity::Working, _) => "working",
                (Activity::Waiting, _) => "waiting",
            };
            self.previous = Some((session.to_owned(), activity));
            // Heartbeats restore renderer activity after reconnects. Only a
            // same-session transition emits completion, never a route change.
            visible.extend_from_slice(
                format!("\x1b]2;{TERMINAL_ACTIVITY_PREFIX}{signal}\x07").as_bytes(),
            );
        }
        visible
    }

    pub(crate) fn finish(&mut self) -> Vec<u8> {
        self.filter.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: &str) -> Vec<u8> {
        format!("\x1b]2;{SNAPSHOT_PREFIX}{payload}\x07").into_bytes()
    }

    #[test]
    fn matches_the_renderer_v1_activity_contract() {
        let mut bridge = OpenCodeActivityBridge::new();
        for line in
            include_str!("../../../electron/src/fixtures/terminal-opencode-activity-v1.txt").lines()
        {
            let (signal, snapshot) = line.split_once('\t').expect("fixture row");
            assert_eq!(
                bridge.feed(&frame(snapshot)),
                format!("\x1b]2;{TERMINAL_ACTIVITY_PREFIX}{signal}\x07").as_bytes()
            );
        }
    }

    #[test]
    fn projects_work_permissions_questions_and_retry_without_output_heuristics() {
        for (status, permissions, questions, expected) in [
            ("busy", 0, 0, Activity::Working),
            ("retry", 0, 0, Activity::Working),
            ("busy", 1, 0, Activity::Waiting),
            ("idle", 0, 1, Activity::Waiting),
            ("idle", 0, 0, Activity::Idle),
        ] {
            let snapshot = format!("ses_a|ses_a,-,{status},{permissions},{questions}");
            assert_eq!(project(&snapshot), Some(("ses_a", expected)));
        }
        assert_eq!(project("-"), Some(("-", Activity::Idle)));
    }

    #[test]
    fn scopes_child_approvals_to_the_visible_root() {
        let rows = "ses_a,-,busy,0,0|ses_child,ses_a,busy,1,0|ses_other,-,busy,1,0";
        assert_eq!(
            project(&format!("ses_a|{rows}")),
            Some(("ses_a", Activity::Waiting))
        );
        assert_eq!(
            project(&format!("ses_child|{rows}")),
            Some(("ses_child", Activity::Working))
        );
        assert_eq!(
            project("ses_a|ses_a,-,busy,0,0|ses_other,-,busy,1,0"),
            Some(("ses_a", Activity::Working))
        );
    }

    #[test]
    fn preserves_split_output_and_only_completes_the_same_session_once() {
        let mut bridge = OpenCodeActivityBridge::new();
        let bytes = frame("ses_a|ses_a,-,busy,0,0");
        let output: Vec<_> = bytes
            .iter()
            .flat_map(|byte| bridge.feed(&[*byte]))
            .collect();
        assert_eq!(output, b"\x1b]2;interface-terminal-activity:v1:working\x07");
        assert_eq!(bridge.feed(b"ordinary output"), b"ordinary output");
        let idle = frame("ses_a|ses_a,-,idle,0,0");
        assert!(
            String::from_utf8(bridge.feed(&idle))
                .unwrap()
                .contains("v1:complete")
        );
        assert!(
            String::from_utf8(bridge.feed(&idle))
                .unwrap()
                .contains("v1:idle")
        );
        bridge.feed(&bytes);
        assert!(
            String::from_utf8(bridge.feed(&frame("ses_b|ses_b,-,idle,0,0")))
                .unwrap()
                .contains("v1:idle")
        );
        assert_eq!(bridge.finish(), b"");
    }

    #[test]
    fn malformed_private_snapshots_never_leak_or_change_activity() {
        for payload in [
            "nope",
            "ses_a",
            "ses_a|ses_a,-,other,0,0",
            "ses_a|ses_a,-,idle,-1,0",
        ] {
            assert!(
                OpenCodeActivityBridge::new()
                    .feed(&frame(payload))
                    .is_empty()
            );
        }
    }
}
