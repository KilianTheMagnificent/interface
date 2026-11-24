//! Native hook contracts for Antigravity, Meta Muse Code and Kimi Code panes.
use interface_common::uuid::canonical_uuid;
use interface_contracts::terminal::TerminalLaunchProfileId as Profile;
use serde_json::Value;

pub const HOOK_MODE: &str = "--terminal-native-checkpoint-hook";
/// Token argument for Muse plugin hooks. A plugin hook runs with a filtered
/// environment that carries no per-pane capture token, so it passes this
/// sentinel and the host resolves the generation from the hook process's
/// ancestry instead. It can never collide with a real capture token, which
/// is always a canonical UUID.
pub const LINEAGE_TOKEN: &str = "lineage";
pub const ACTIVITY_PREFIX: &str = crate::TERMINAL_ACTIVITY_PREFIX;
pub const TOKEN_ENV: &str = "INTERFACE_NATIVE_CAPTURE_TOKEN";
pub const ROOT_ENV: &str = "INTERFACE_NATIVE_RECEIPTS_ROOT";
pub const EXE_ENV: &str = "INTERFACE_NATIVE_HOOK_EXE";

#[must_use]
pub fn supports(profile: Profile) -> bool {
    matches!(profile, Profile::Gemini | Profile::Muse | Profile::Kimi)
}

/// The prefix distinguishes provider proof from legacy renderer-minted UUIDs.
#[must_use]
pub fn session_id(profile: Profile, binding: &str) -> Option<&str> {
    let (prefix, version) = match profile {
        Profile::Gemini => ("agy-", b'4'),
        Profile::Muse => ("muse-", b'7'),
        Profile::Kimi => ("kimi-", b'4'),
        _ => return None,
    };
    binding
        .strip_prefix(prefix)
        .filter(|id| canonical_uuid(id, version))
}

/// Kimi Code names a session `session_<v4 UUID>`; the binding keeps only the
/// UUID behind its own prefix, like the other native providers.
const KIMI_SESSION_PREFIX: &str = "session_";

/// The Kimi Code lifecycle events Interface's plugin observes. One list feeds
/// the plugin manifest and [`event`], so the two can never drift apart.
pub const KIMI_HOOK_EVENTS: [&str; 13] = [
    "SessionStart",
    "UserPromptSubmit",
    "TurnStarted",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionResult",
    "SubagentStart",
    "SubagentStop",
    "Stop",
    "StopFailure",
    "Interrupt",
];

/// The id `kimi --session` takes for a `kimi-<uuid>` binding.
#[must_use]
pub fn kimi_session_argument(binding: &str) -> Option<String> {
    session_id(Profile::Kimi, binding).map(|id| format!("{KIMI_SESSION_PREFIX}{id}"))
}

#[derive(Debug, PartialEq, Eq)]
pub struct HookEvent {
    pub binding: String,
    pub cwd: Vec<String>,
    pub activity: &'static str,
    pub source: String,
}

/// Unknown events and incomplete stops never establish idle evidence.
#[must_use]
pub fn event(profile: Profile, name: &str, value: &Value) -> Option<HookEvent> {
    let (prefix, id, cwd, activity, source) = match profile {
        Profile::Gemini => {
            let id = value.get("conversationId")?.as_str()?;
            let cwd = value
                .get("workspacePaths")?
                .as_array()?
                .iter()
                .map(|path| path.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?;
            let activity = match name {
                "Stop"
                    if value.get("fullyIdle").and_then(Value::as_bool) == Some(true)
                        && value
                            .get("error")
                            .and_then(Value::as_str)
                            .is_none_or(str::is_empty) =>
                {
                    "idle"
                }
                "Stop" | "SessionStart" | "PreInvocation" => "working",
                _ => return None,
            };
            ("agy-", id, cwd, activity, "startup")
        }
        Profile::Muse => {
            if value.get("hook_event_name")?.as_str()? != name
                || (name != "Notification"
                    && value.get("post_stop").and_then(Value::as_bool) == Some(true))
            {
                return None;
            }
            let id = value.get("session_id")?.as_str()?;
            let cwd = vec![value.get("cwd")?.as_str()?.to_owned()];
            let activity = match name {
                "SessionStart" | "SessionEnd" | "UserPromptSubmit" | "PreToolUse"
                | "PostToolUse" | "PostToolUseFailure" | "PreLLMCall" | "SubagentStart"
                | "SubagentStop" => "working",
                "PermissionRequest" | "Notification" => "waiting",
                "Stop" | "StopFailure" | "Interrupt" => "idle",
                _ => return None,
            };
            (
                "muse-",
                id,
                cwd,
                activity,
                value
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("startup"),
            )
        }
        Profile::Kimi => {
            if value.get("hook_event_name")?.as_str()? != name {
                return None;
            }
            let id = value
                .get("session_id")?
                .as_str()?
                .strip_prefix(KIMI_SESSION_PREFIX)?;
            let activity = kimi_activity(name, value)?;
            // Kimi's payload carries no cwd (the hook runs in the plugin's
            // root); the pane's capture token and process lineage bind it.
            // 2.3.1 starts sessions only as `startup` (first prompt, `/new`)
            // or `resume`; an unknown source never reaches a receipt.
            let source = value
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("startup");
            if !matches!(source, "startup" | "resume") {
                return None;
            }
            ("kimi-", id, Vec::new(), activity, source)
        }
        _ => return None,
    };
    let binding = format!("{prefix}{id}");
    session_id(profile, &binding)?;
    if (cwd.is_empty() && profile != Profile::Kimi)
        || cwd
            .iter()
            .any(|path| !std::path::Path::new(path).is_absolute())
    {
        return None;
    }
    Some(HookEvent {
        binding,
        cwd,
        activity,
        source: source.to_owned(),
    })
}

/// The activity one Kimi Code hook reports before `Activity` weighs it.
fn kimi_activity(name: &str, value: &Value) -> Option<&'static str> {
    Some(match name {
        // Its question tool waits on the user like an approval.
        "PreToolUse"
            if value.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion") =>
        {
            "waiting"
        }
        "UserPromptSubmit" | "TurnStarted" | "PreToolUse" | "PostToolUse"
        | "PostToolUseFailure" | "PermissionResult" | "SubagentStart" | "SubagentStop" => "working",
        "PermissionRequest" => "waiting",
        // Kimi creates its session on first use, and `/new` or a session
        // switch starts another; none of them is work yet.
        "SessionStart" | "Stop" | "StopFailure" | "Interrupt" => "idle",
        _ => return None,
    })
}

/// Native children can finish after the root's `Stop`. A child's `Stop` may be
/// vetoed by another hook; only its `SessionEnd` releases the barrier.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activity {
    children: std::collections::BTreeSet<String>,
    children_unknown: bool,
    root_stopped: bool,
    /// Kimi names no child session, so its open subagent tasks are counted.
    #[serde(default, skip_serializing_if = "is_zero")]
    open_children: u8,
    /// Kimi's last answer was work, so its next idle finishes a turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    busy: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if signature
fn is_zero(value: &u8) -> bool {
    *value == 0
}
impl Activity {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.children.len() <= 64
            && self
                .children
                .iter()
                .all(|id| !id.is_empty() && id.len() <= 128)
    }

    pub fn observe(
        &mut self,
        profile: Profile,
        name: &str,
        activity: &'static str,
        child: Option<&str>,
    ) -> &'static str {
        if profile == Profile::Kimi {
            return self.observe_kimi(name, activity);
        }
        if profile != Profile::Muse {
            return activity;
        }
        match name {
            "SubagentStart" => {
                if let Some(id) = child.filter(|id| !id.is_empty() && id.len() <= 128) {
                    if self.children.contains(id) || self.children.len() < 64 {
                        self.children.insert(id.into());
                    } else {
                        self.children_unknown = true;
                    }
                } else {
                    self.children_unknown = true;
                }
            }
            "SubagentStop" => {}
            "SessionEnd" if child.is_some() => {
                self.children.remove(child.unwrap_or_default());
            }
            "Stop" => self.root_stopped = true,
            "StopFailure" | "Interrupt" => {
                self.children.clear();
                self.children_unknown = false;
                self.root_stopped = true;
            }
            _ => self.root_stopped = false,
        }
        if activity == "waiting" {
            "waiting"
        } else if self.root_stopped && self.children.is_empty() && !self.children_unknown {
            "idle"
        } else {
            "working"
        }
    }

    /// Every Kimi agent, subagents included, reports under the root session
    /// id, so a subagent's own `Stop` arrives while its task is open: only a
    /// `Stop` with no subagent task open ends the pane's turn. The idle that
    /// follows work is reported as `complete`, the finished-turn signal.
    fn observe_kimi(&mut self, name: &str, activity: &'static str) -> &'static str {
        match name {
            "SubagentStart" => {
                if self.open_children < 64 {
                    self.open_children += 1;
                } else {
                    self.children_unknown = true;
                }
            }
            "SubagentStop" => self.open_children = self.open_children.saturating_sub(1),
            "Stop" => {
                if self.open_children == 0 && !self.children_unknown {
                    self.root_stopped = true;
                }
            }
            "StopFailure" | "Interrupt" | "SessionStart" => {
                self.open_children = 0;
                self.children_unknown = false;
                self.root_stopped = true;
            }
            _ => self.root_stopped = false,
        }
        let state = if activity == "waiting" {
            "waiting"
        } else if self.root_stopped && self.open_children == 0 && !self.children_unknown {
            "idle"
        } else {
            "working"
        };
        // The user's own interrupt is no finished turn to point them at.
        let finished =
            state == "idle" && self.busy && !matches!(name, "SessionStart" | "Interrupt");
        self.busy = state != "idle";
        if finished { "complete" } else { state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn muse_waits_for_child_end_and_releases_only_after_the_root_has_stopped() {
        let mut state = Activity::default();
        assert_eq!(
            state.observe(Profile::Muse, "SubagentStart", "working", Some("child")),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "Stop", "idle", None),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "SubagentStop", "working", Some("child")),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "SessionEnd", "working", Some("child")),
            "idle"
        );
        assert_eq!(
            state.observe(Profile::Muse, "PreLLMCall", "working", None),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "PermissionRequest", "waiting", None),
            "waiting"
        );
        assert_eq!(
            state.observe(Profile::Muse, "SubagentStart", "working", None),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "Stop", "idle", None),
            "working"
        );
    }
    #[test]
    fn antigravity_and_muse_require_native_ids_and_real_stop_events() {
        let mut agy = json!({"conversationId":"d2d9dfef-c119-4df6-8827-2bad4e79eda6", "workspacePaths":["/tmp/project"]});
        assert_eq!(
            event(Profile::Gemini, "Stop", &agy).unwrap().activity,
            "working"
        );
        agy["fullyIdle"] = json!(true);
        assert_eq!(
            event(Profile::Gemini, "Stop", &agy).unwrap().activity,
            "idle"
        );
        assert!(event(Profile::Gemini, "PostInvocation", &agy).is_none());
        assert!(session_id(Profile::Gemini, "d2d9dfef-c119-4df6-8827-2bad4e79eda6").is_none());
        let muse = json!({"session_id":"01a15add-7a55-74d1-bf33-5d8d5502c449","cwd":"/tmp/project","hook_event_name":"Stop"});
        assert_eq!(
            event(Profile::Muse, "Stop", &muse).unwrap().activity,
            "idle"
        );
        assert!(event(Profile::Muse, "UserPromptSubmit", &muse).is_none());
        assert!(session_id(Profile::Gemini, "muse-01a15add-7a55-74d1-bf33-5d8d5502c449").is_none());
    }
    #[test]
    fn muse_stop_failure_and_interrupt_release_to_idle_and_ignore_post_stop_noise() {
        let stop_failure = json!({
            "session_id": "01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "cwd": "/tmp/project",
            "hook_event_name": "StopFailure"
        });
        assert_eq!(
            event(Profile::Muse, "StopFailure", &stop_failure)
                .unwrap()
                .activity,
            "idle"
        );
        let interrupt = json!({
            "session_id": "01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "cwd": "/tmp/project",
            "hook_event_name": "Interrupt"
        });
        assert_eq!(
            event(Profile::Muse, "Interrupt", &interrupt)
                .unwrap()
                .activity,
            "idle"
        );
        let mut state = Activity::default();
        assert_eq!(
            state.observe(Profile::Muse, "PreLLMCall", "working", None),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "SubagentStart", "working", Some("reminder")),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "StopFailure", "idle", None),
            "idle"
        );
        assert_eq!(
            state.observe(Profile::Muse, "PermissionRequest", "waiting", None),
            "waiting"
        );
        assert_eq!(
            state.observe(Profile::Muse, "PostToolUse", "working", None),
            "working"
        );
        assert_eq!(
            state.observe(Profile::Muse, "Interrupt", "idle", None),
            "idle"
        );
        let stale_permission = json!({
            "session_id": "01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "cwd": "/tmp/project",
            "hook_event_name": "PermissionRequest",
            "post_stop": true
        });
        assert!(event(Profile::Muse, "PermissionRequest", &stale_permission).is_none());
        let live_notification = json!({
            "session_id": "01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "cwd": "/tmp/project",
            "hook_event_name": "Notification",
            "post_stop": true
        });
        assert_eq!(
            event(Profile::Muse, "Notification", &live_notification)
                .unwrap()
                .activity,
            "waiting"
        );
    }

    // Kimi Code 2.3.1 sends `hook_event_name` and `session_id`
    // (`session_<v4 UUID>`) with no cwd, and runs every agent's hooks under
    // the root session, so subagent tasks bracket their own stops.
    #[test]
    fn kimi_binds_its_session_and_ends_a_turn_only_when_no_subagent_is_open() {
        let id = "session_3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73";
        let hook = |name: &str| json!({"hook_event_name": name, "session_id": id});
        let start = event(Profile::Kimi, "SessionStart", &hook("SessionStart")).unwrap();
        assert_eq!(start.binding, "kimi-3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73");
        assert!(start.cwd.is_empty());
        assert_eq!(start.source, "startup");
        assert_eq!(kimi_session_argument(&start.binding).as_deref(), Some(id));
        assert!(event(Profile::Kimi, "Stop", &hook("SessionStart")).is_none());
        assert!(
            event(
                Profile::Kimi,
                "Stop",
                &json!({"hook_event_name":"Stop","session_id":"3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73"})
            )
            .is_none()
        );
        assert!(event(Profile::Kimi, "SessionHeartbeat", &hook("SessionHeartbeat")).is_none());
        let mut unknown = hook("SessionStart");
        unknown["source"] = json!("reload");
        assert!(event(Profile::Kimi, "SessionStart", &unknown).is_none());
        for name in KIMI_HOOK_EVENTS {
            assert!(event(Profile::Kimi, name, &hook(name)).is_some(), "{name}");
        }
        let mut question = hook("PreToolUse");
        question["tool_name"] = json!("AskUserQuestion");
        assert_eq!(
            event(Profile::Kimi, "PreToolUse", &question)
                .unwrap()
                .activity,
            "waiting"
        );
        assert!(session_id(Profile::Kimi, "kimi-01a15add-7a55-74d1-bf33-5d8d5502c449").is_none());
        assert!(session_id(Profile::Muse, &start.binding).is_none());

        let mut state = Activity::default();
        let mut observe = |name: &str| {
            let activity = event(Profile::Kimi, name, &hook(name)).unwrap().activity;
            state.observe(Profile::Kimi, name, activity, None)
        };
        assert_eq!(observe("SessionStart"), "idle");
        assert_eq!(observe("UserPromptSubmit"), "working");
        assert_eq!(observe("SubagentStart"), "working");
        assert_eq!(observe("Stop"), "working", "a subagent's own stop");
        assert_eq!(observe("SubagentStop"), "working");
        assert_eq!(observe("PermissionRequest"), "waiting");
        assert_eq!(observe("PermissionResult"), "working");
        assert_eq!(observe("Stop"), "complete");
        assert_eq!(observe("Stop"), "idle", "a repeated stop is no new turn");
        assert_eq!(observe("UserPromptSubmit"), "working");
        assert_eq!(observe("SubagentStart"), "working");
        assert_eq!(observe("Interrupt"), "idle");
        assert_eq!(observe("SessionStart"), "idle");
    }

    #[test]
    fn kimi_activity_fields_stay_out_of_other_providers_receipts() {
        let mut state = Activity::default();
        state.observe(Profile::Muse, "PreLLMCall", "working", None);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({"children": [], "childrenUnknown": false, "rootStopped": false})
        );
        state.observe(Profile::Kimi, "SubagentStart", "working", None);
        let saved = serde_json::to_value(&state).unwrap();
        assert_eq!(saved["openChildren"], json!(1));
        assert_eq!(saved["busy"], json!(true));
        let restored: Activity = serde_json::from_value(saved).unwrap();
        assert!(restored.valid());
    }
}
