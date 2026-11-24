//! An agent CLI that ends on its own hands its pane back to a plain shell in
//! the folder it worked in, so the position stays usable instead of holding
//! a dead tab (host protocol v50). Rust decides it when the exit lands,
//! whatever the window shows: a Codex update that finishes while its
//! workspace is parked gets its shell too. The renderer only moves the tab;
//! the shell's summary names the agent it `replaces`.
//!
//! Deliberately narrow. A crash, a signal, a launch that never started and an
//! exit someone asked for (closing the tab, deleting the workspace, quitting)
//! keep their tab, so the output stays readable and Start again still works.

use interface_contracts::terminal::{
    CreateTerminalRequest, TERMINAL_LAUNCH_REQUEST_VERSION, TerminalId, TerminalLaunchProfileId,
    TerminalLaunchRequestV1, TerminalState, TerminalSummary,
};

/// Whether `terminal` is an agent CLI that has just exited cleanly: worth
/// asking the service whether nobody asked it to close.
#[must_use]
pub fn ended_on_its_own_terms(terminal: &TerminalSummary) -> bool {
    terminal.state == TerminalState::Exited
        && terminal
            .exit
            .as_ref()
            .is_some_and(|exit| exit.code == Some(0))
        && terminal
            .launch
            .is_some_and(|launch| launch.profile_id != TerminalLaunchProfileId::Shell)
        && !terminal.cwd.trim().is_empty()
}

/// The shell to start under the fresh `id` in `exited`'s place: a plain
/// shell, at the agent's size, in the folder the agent started in.
#[must_use]
pub fn shell_handoff_request(exited: &TerminalSummary, id: TerminalId) -> CreateTerminalRequest {
    CreateTerminalRequest {
        id,
        cwd: exited.cwd.clone(),
        size: exited.size,
        title: None,
        session_key: None,
        launch: Some(TerminalLaunchRequestV1 {
            version: TERMINAL_LAUNCH_REQUEST_VERSION,
            profile_id: TerminalLaunchProfileId::Shell,
        }),
        launch_session: None,
        handoff: None,
        sign_in: false,
        account: None,
    }
}

#[cfg(test)]
mod tests {
    use interface_contracts::terminal::{TerminalExit, TerminalGeneration, TerminalSize};

    use super::*;

    fn agent(profile_id: TerminalLaunchProfileId) -> TerminalSummary {
        TerminalSummary {
            id: TerminalId::random(),
            generation: TerminalGeneration::initial(),
            title: "Codex".to_owned(),
            root: "/tmp/project".to_owned(),
            cwd: "/tmp/project/crates".to_owned(),
            session_key: None,
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id,
            }),
            launch_session: None,
            state: TerminalState::Exited,
            size: TerminalSize {
                columns: 132,
                rows: 41,
            },
            created_at: "2026-10-14T00:00:00.000Z".to_owned(),
            updated_at: "2026-10-14T00:00:00.000Z".to_owned(),
            exit: Some(TerminalExit {
                code: Some(0),
                signal: None,
            }),
            passed_off_from: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            replaces: None,
        }
    }

    #[test]
    fn only_an_agent_that_exits_cleanly_is_handed_a_shell_where_it_worked() {
        let codex = agent(TerminalLaunchProfileId::Codex);
        assert!(ended_on_its_own_terms(&codex));
        let id = TerminalId::random();
        let shell = shell_handoff_request(&codex, id);
        assert_eq!(shell.id, id);
        assert_eq!(shell.cwd, "/tmp/project/crates");
        assert_eq!(shell.size, codex.size);
        assert_eq!(
            shell.launch.map(|launch| launch.profile_id),
            Some(TerminalLaunchProfileId::Shell)
        );
        assert!(shell.handoff.is_none() && shell.launch_session.is_none() && !shell.sign_in);

        // A crash, a signal, a failed start, a shell and a running agent keep their tab.
        let mut crashed = codex.clone();
        crashed.exit = Some(TerminalExit {
            code: Some(3),
            signal: None,
        });
        let mut signalled = codex.clone();
        signalled.exit = Some(TerminalExit {
            code: None,
            signal: Some(9),
        });
        let mut failed = codex.clone();
        failed.state = TerminalState::Failed;
        let mut running = codex.clone();
        running.state = TerminalState::Running;
        running.exit = None;
        let mut no_folder = codex.clone();
        no_folder.cwd = " ".to_owned();
        let mut unlaunched = codex;
        unlaunched.launch = None;
        for kept in [
            crashed,
            signalled,
            failed,
            running,
            no_folder,
            unlaunched,
            agent(TerminalLaunchProfileId::Shell),
        ] {
            assert!(!ended_on_its_own_terms(&kept), "{kept:?}");
        }
    }
}
