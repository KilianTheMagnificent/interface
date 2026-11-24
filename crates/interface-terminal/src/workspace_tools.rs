//! Native utilities launched from a terminal workspace with its existing root authority.
use crate::{
    ApprovedTerminalRoots,
    safety::{assert_directory, root_error},
};
use interface_common::process::CapturedPipe;
use interface_contracts::workspace_tools::{OpenWorkspaceToolV1, WorkspaceTool};
use serde_json::Value;
use std::{
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// A cold app launch on a loaded machine outlasts a few seconds.
const OPEN_DEADLINE: Duration = Duration::from_secs(15);
/// The simulator service starts on first use.
const SIMCTL_LIST_DEADLINE: Duration = Duration::from_secs(30);
const SIMCTL_BOOT_DEADLINE: Duration = Duration::from_secs(90);
const OUTPUT_LIMIT: usize = 1024 * 1024;
const NEEDS_XCODE: &str =
    "iOS Simulator needs Xcode. Install it from the App Store and open it once to finish setup.";

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceToolError {
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    OutcomeUnknown(String),
}

pub trait WorkspaceToolLauncher {
    /// # Errors
    /// Reports an unsent launch separately from one whose outcome is unknown.
    fn open(&self, arguments: &[String]) -> Result<(), WorkspaceToolError>;
    /// `simctl list devices available -j`, read-only.
    /// # Errors
    /// Refuses when Xcode's simulator tools are missing or do not answer.
    fn simulator_devices(&self) -> Result<String, WorkspaceToolError>;
    /// # Errors
    /// Refuses a boot `simctl` declined; an unfinished boot is an unknown outcome.
    fn boot_simulator(&self, udid: &str) -> Result<(), WorkspaceToolError>;
}

pub struct SystemWorkspaceToolLauncher;
impl WorkspaceToolLauncher for SystemWorkspaceToolLauncher {
    fn open(&self, arguments: &[String]) -> Result<(), WorkspaceToolError> {
        match run_bounded("/usr/bin/open", arguments, OPEN_DEADLINE)? {
            Some(finished) if finished.success => Ok(()),
            Some(_) => Err(WorkspaceToolError::OutcomeUnknown(
                "macOS could not confirm opening the app. Check whether it opened before trying again.".into(),
            )),
            None => Err(WorkspaceToolError::OutcomeUnknown(
                "The app launch did not finish in time. Check whether it opened before trying again.".into(),
            )),
        }
    }

    fn simulator_devices(&self) -> Result<String, WorkspaceToolError> {
        // xcode-select answers without the Command Line Tools install prompt
        // that a bare xcrun raises on a Mac with no developer tools at all.
        let selected = run_bounded(
            "/usr/bin/xcode-select",
            &["-p".into()],
            Duration::from_secs(5),
        )?;
        if !selected.is_some_and(|finished| finished.success) {
            return Err(WorkspaceToolError::Refused(NEEDS_XCODE.into()));
        }
        let arguments = ["simctl", "list", "devices", "available", "-j"].map(String::from);
        match run_bounded("/usr/bin/xcrun", &arguments, SIMCTL_LIST_DEADLINE)? {
            Some(finished) if finished.success => String::from_utf8(finished.stdout)
                .map_err(|_| WorkspaceToolError::Refused(NEEDS_XCODE.into())),
            Some(_) => Err(WorkspaceToolError::Refused(NEEDS_XCODE.into())),
            None => Err(WorkspaceToolError::Refused(
                "Xcode's simulator service did not answer in time. Try again in a moment.".into(),
            )),
        }
    }

    fn boot_simulator(&self, udid: &str) -> Result<(), WorkspaceToolError> {
        let arguments = ["simctl".into(), "boot".into(), udid.to_owned()];
        match run_bounded("/usr/bin/xcrun", &arguments, SIMCTL_BOOT_DEADLINE)? {
            Some(finished) if finished.success => Ok(()),
            // Something else booted it between the listing and now.
            Some(finished)
                if String::from_utf8_lossy(&finished.stderr).contains("current state: Booted") =>
            {
                Ok(())
            }
            Some(_) => Err(WorkspaceToolError::Refused(
                "The iPhone simulator could not start. Open Xcode once to finish its setup, then try again.".into(),
            )),
            None => Err(WorkspaceToolError::OutcomeUnknown(
                "The iPhone simulator did not finish starting in time. Check Simulator before trying again.".into(),
            )),
        }
    }
}

struct Finished {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Runs fixed argv without a shell. `None` means it outlived `limit` and was killed.
fn run_bounded(
    program: &str,
    arguments: &[String],
    limit: Duration,
) -> Result<Option<Finished>, WorkspaceToolError> {
    if !cfg!(target_os = "macos") {
        return Err(WorkspaceToolError::Refused(
            "Native workspace tools require macOS.".into(),
        ));
    }
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            WorkspaceToolError::Refused("macOS could not start the application launcher.".into())
        })?;
    let overflow = Arc::new(AtomicBool::new(false));
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };
    let stdout = CapturedPipe::spawn(stdout, OUTPUT_LIMIT, Arc::clone(&overflow));
    let stderr = CapturedPipe::spawn(stderr, 16 * 1024, Arc::new(AtomicBool::new(false)));
    let deadline = Instant::now() + limit;
    loop {
        let observed = child.try_wait();
        if let Ok(Some(_)) = observed {
            stdout.child_exited(None);
            stderr.child_exited(None);
        }
        match observed {
            Ok(Some(status)) if stdout.is_finished() && stderr.is_finished() => {
                return Ok(Some(Finished {
                    success: status.success() && !overflow.load(Ordering::Relaxed),
                    stdout: stdout.finish().unwrap_or_default(),
                    stderr: stderr.finish().unwrap_or_default(),
                }));
            }
            Ok(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(None);
            }
        }
    }
}

/// One available iPhone simulator from `simctl`'s listing.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Iphone {
    udid: String,
    booted: bool,
    /// `simctl`'s UTC ISO-8601 stamp, which sorts as text.
    last_used: Option<String>,
    /// The iOS runtime's (major, minor).
    runtime: (u32, u32),
}

fn available_iphones(listing: &str) -> Vec<Iphone> {
    let Ok(value) = serde_json::from_str::<Value>(listing) else {
        return Vec::new();
    };
    let Some(runtimes) = value.get("devices").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut phones = Vec::new();
    for (runtime, devices) in runtimes {
        let Some(version) = runtime.strip_prefix("com.apple.CoreSimulator.SimRuntime.iOS-") else {
            continue;
        };
        let mut parts = version
            .split('-')
            .map(|part| part.parse::<u32>().unwrap_or(0));
        let runtime = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        for device in devices.as_array().into_iter().flatten() {
            let text = |key: &str| device.get(key).and_then(Value::as_str);
            let (Some(udid), Some(kind)) = (text("udid"), text("deviceTypeIdentifier")) else {
                continue;
            };
            if !kind.contains(".iPhone")
                || device.get("isAvailable").and_then(Value::as_bool) == Some(false)
                || !is_udid(udid)
            {
                continue;
            }
            phones.push(Iphone {
                udid: udid.to_owned(),
                booted: text("state") == Some("Booted"),
                last_used: text("lastUsedAt").map(str::to_owned),
                runtime,
            });
        }
    }
    phones
}

/// A booted iPhone first, then the one used last, then the newest runtime's
/// first listed model.
fn preferred_iphone(phones: &[Iphone]) -> Option<&Iphone> {
    // `max_by_key` keeps the last of equals; reversed, that is the first listed.
    phones
        .iter()
        .rev()
        .max_by_key(|phone| (phone.booted, phone.last_used.as_deref(), phone.runtime))
}

fn is_udid(value: &str) -> bool {
    value.len() == 36 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Open Finder, the default browser or Apple's Simulator using fixed argv.
/// Simulator starts on an iPhone: a booted one, else the one used last.
/// # Errors
/// Rejects unapproved or changed folders before effects; never retries a launch.
pub fn open_workspace_tool(
    request: &OpenWorkspaceToolV1,
    approved: &ApprovedTerminalRoots,
    launcher: &dyn WorkspaceToolLauncher,
) -> Result<(), WorkspaceToolError> {
    request
        .validate()
        .map_err(|e| WorkspaceToolError::Refused(e.to_string()))?;
    let root = approved
        .authorize_pass_off(Path::new(&request.root))
        .map_err(|e| WorkspaceToolError::Refused(root_error(e).to_string()))?;
    let check_folder = || {
        assert_directory(&root.root)
            .and_then(|()| assert_directory(&root.cwd))
            .map_err(|_| {
                WorkspaceToolError::Refused(
                    "The workspace folder changed. Reopen it before continuing.".into(),
                )
            })
    };
    let arguments = match request.tool {
        WorkspaceTool::Files => vec!["-a".into(), "Finder".into(), request.root.clone()],
        WorkspaceTool::Browser => vec![request.url.clone().unwrap_or_default()],
        WorkspaceTool::IosSimulator => {
            check_folder()?;
            let phones = available_iphones(&launcher.simulator_devices()?);
            let Some(phone) = preferred_iphone(&phones) else {
                return Err(WorkspaceToolError::Refused(
                    "Xcode has no iPhone simulator. Add an iOS platform in Xcode › Settings › Components.".into(),
                ));
            };
            if !phone.booted {
                launcher.boot_simulator(&phone.udid)?;
            }
            // A fresh Simulator opens on this device; a running one already
            // shows it, since it is booted.
            vec![
                "-a".into(),
                "Simulator".into(),
                "--args".into(),
                "-CurrentDeviceUDID".into(),
                phone.udid.clone(),
            ]
        }
    };
    check_folder()?;
    launcher.open(&arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, fs};

    const PRO: &str = "11111111-1111-1111-1111-111111111111";
    const AIR: &str = "22222222-2222-2222-2222-222222222222";
    const IPAD: &str = "33333333-3333-3333-3333-333333333333";

    fn listing(pro_state: &str, air_last_used: Option<&str>) -> String {
        serde_json::json!({ "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-26-5": [
                { "udid": AIR, "name": "iPhone Air", "state": "Shutdown", "isAvailable": true,
                  "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-Air",
                  "lastUsedAt": air_last_used },
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                { "udid": IPAD, "name": "iPad Pro", "state": "Booted", "isAvailable": true,
                  "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPad-Pro" },
                { "udid": PRO, "name": "iPhone 18 Pro", "state": pro_state, "isAvailable": true,
                  "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro" },
                { "udid": "not-a-udid", "name": "iPhone 18", "state": "Booted", "isAvailable": true,
                  "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-18" },
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-12-0": [
                { "udid": "44444444-4444-4444-4444-444444444444", "name": "Watch", "state": "Booted",
                  "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.Apple-Watch" },
            ],
        }})
        .to_string()
    }

    #[derive(Default)]
    struct Fake {
        opened: RefCell<Vec<Vec<String>>>,
        booted: RefCell<Vec<String>>,
        listing: String,
    }
    impl WorkspaceToolLauncher for Fake {
        fn open(&self, args: &[String]) -> Result<(), WorkspaceToolError> {
            self.opened.borrow_mut().push(args.to_vec());
            Ok(())
        }
        fn simulator_devices(&self) -> Result<String, WorkspaceToolError> {
            Ok(self.listing.clone())
        }
        fn boot_simulator(&self, udid: &str) -> Result<(), WorkspaceToolError> {
            self.booted.borrow_mut().push(udid.to_owned());
            Ok(())
        }
    }

    #[test]
    fn prefers_a_booted_iphone_then_the_last_used_then_the_newest_runtime() {
        let pick =
            |text: &str| preferred_iphone(&available_iphones(text)).map(|phone| phone.udid.clone());
        assert_eq!(
            pick(&listing("Booted", Some("2026-09-20T10:00:00Z"))).as_deref(),
            Some(PRO)
        );
        assert_eq!(
            pick(&listing("Shutdown", Some("2026-09-20T10:00:00Z"))).as_deref(),
            Some(AIR)
        );
        assert_eq!(pick(&listing("Shutdown", None)).as_deref(), Some(PRO));
        assert_eq!(pick(r#"{"devices":{}}"#), None);
        assert_eq!(pick("not json"), None);
    }

    #[test]
    fn routes_fixed_argv_without_shell_and_refuses_outside_roots() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let nested = root.join("nested ' $(false)");
        fs::create_dir(&nested).unwrap();
        let approved = ApprovedTerminalRoots::new([root.clone()]).unwrap();
        let launcher = Fake {
            listing: listing("Shutdown", None),
            ..Fake::default()
        };
        let mut request = OpenWorkspaceToolV1 {
            version: 1,
            root: nested.to_str().unwrap().into(),
            tool: WorkspaceTool::Files,
            url: None,
        };
        open_workspace_tool(&request, &approved, &launcher).unwrap();
        assert_eq!(
            launcher.opened.borrow()[0],
            ["-a", "Finder", request.root.as_str()]
        );
        request.tool = WorkspaceTool::IosSimulator;
        open_workspace_tool(&request, &approved, &launcher).unwrap();
        assert_eq!(*launcher.booted.borrow(), [PRO]);
        assert_eq!(
            launcher.opened.borrow()[1],
            ["-a", "Simulator", "--args", "-CurrentDeviceUDID", PRO]
        );
        request.tool = WorkspaceTool::Browser;
        request.url = Some("http://localhost:3000".into());
        open_workspace_tool(&request, &approved, &launcher).unwrap();
        assert_eq!(launcher.opened.borrow()[2], ["http://localhost:3000"]);
        request.root = "/".into();
        assert!(open_workspace_tool(&request, &approved, &launcher).is_err());
        assert_eq!(launcher.opened.borrow().len(), 3);
    }

    #[test]
    fn a_booted_iphone_is_shown_without_booting_and_none_is_refused_unsent() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let approved = ApprovedTerminalRoots::new([root.clone()]).unwrap();
        let request = OpenWorkspaceToolV1 {
            version: 1,
            root: root.to_str().unwrap().into(),
            tool: WorkspaceTool::IosSimulator,
            url: None,
        };
        let booted = Fake {
            listing: listing("Booted", None),
            ..Fake::default()
        };
        open_workspace_tool(&request, &approved, &booted).unwrap();
        assert!(booted.booted.borrow().is_empty());
        assert_eq!(
            booted.opened.borrow()[0].last().map(String::as_str),
            Some(PRO)
        );

        let empty = Fake {
            listing: r#"{"devices":{}}"#.into(),
            ..Fake::default()
        };
        let error = open_workspace_tool(&request, &approved, &empty).unwrap_err();
        assert!(
            matches!(error, WorkspaceToolError::Refused(message) if message.contains("no iPhone simulator"))
        );
        assert!(empty.opened.borrow().is_empty() && empty.booted.borrow().is_empty());
    }
}
