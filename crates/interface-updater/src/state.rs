use interface_contracts::updater::{
    MAX_UPDATE_ERROR_LENGTH, MAX_UPDATE_VERSION_LENGTH, UpdatePhase, UpdateSource, UpdateState,
    initial_update_state,
};

use crate::NormalizedFeed;

pub const REFUSED_UPDATE_MESSAGE: &str =
    "Interface could not safely verify the downloaded update. It was not installed.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerificationTicket(u64);

#[derive(Clone, Debug)]
pub struct UpdateMachine {
    packaged: bool,
    feed: Option<NormalizedFeed>,
    state: UpdateState,
    verification_generation: u64,
}

impl UpdateMachine {
    #[must_use]
    pub fn new(current_version: &str, packaged: bool) -> Self {
        Self {
            packaged,
            feed: None,
            state: initial_update_state(current_version),
            verification_generation: 0,
        }
    }

    #[must_use]
    pub fn state(&self) -> UpdateState {
        self.state.clone()
    }

    pub fn set_feed(&mut self, feed: Option<NormalizedFeed>) {
        self.verification_generation = self.verification_generation.wrapping_add(1);
        self.feed = feed;
        self.patch(
            if let Some(feed) = &self.feed {
                (
                    if self.packaged {
                        UpdatePhase::Idle
                    } else {
                        UpdatePhase::Disabled
                    },
                    feed.source(),
                )
            } else {
                (UpdatePhase::Disabled, UpdateSource::None)
            },
            None,
            None,
            None,
        );
    }

    pub fn checking(&mut self) {
        if self.enabled() && self.state.phase != UpdatePhase::Ready {
            self.patch_current(UpdatePhase::Checking, None, None);
        }
    }

    pub fn available(&mut self, version: Option<&str>, checked_at: &str) {
        if self.enabled() && self.state.phase != UpdatePhase::Ready {
            let Some(candidate) = safe_version(version) else {
                self.not_available(checked_at);
                return;
            };
            if !is_newer_version(candidate, &self.state.current_version) {
                self.not_available(checked_at);
                return;
            }
            self.state.last_checked_at = Some(checked_at.to_owned());
            self.patch_current(UpdatePhase::Available, Some(candidate.to_owned()), None);
        }
    }

    pub fn not_available(&mut self, checked_at: &str) {
        if self.enabled() && self.state.phase != UpdatePhase::Ready {
            self.state.last_checked_at = Some(checked_at.to_owned());
            self.patch_current(UpdatePhase::UpToDate, None, None);
        }
    }

    pub fn progress(&mut self, progress: Option<f64>) {
        if self.enabled() && self.state.phase != UpdatePhase::Ready {
            let progress = progress
                .filter(|value| value.is_finite())
                .map(|value| value.clamp(0.0, 100.0))
                .or(self.state.progress);
            self.patch_current(
                UpdatePhase::Downloading,
                self.state.available_version.clone(),
                progress,
            );
        }
    }

    /// Begins the signature gate for a downloaded artifact.
    #[must_use]
    pub fn downloaded_requiring_verification(
        &mut self,
        version: Option<&str>,
        artifact_present: bool,
        checked_at: &str,
    ) -> Option<VerificationTicket> {
        if !self.enabled() {
            return None;
        }
        let version = safe_version(version)
            .map(str::to_owned)
            .or_else(|| self.state.available_version.clone());
        if self.state.phase == UpdatePhase::Ready && version == self.state.available_version {
            return None;
        }
        self.verification_generation = self.verification_generation.wrapping_add(1);
        if !artifact_present
            || version
                .as_deref()
                .is_none_or(|candidate| !is_newer_version(candidate, &self.state.current_version))
        {
            self.refuse(version, checked_at);
            return None;
        }
        self.patch_current(UpdatePhase::Downloading, version, Some(100.0));
        Some(VerificationTicket(self.verification_generation))
    }

    pub fn complete_verification(
        &mut self,
        ticket: VerificationTicket,
        accepted: bool,
        checked_at: &str,
    ) {
        if !self.enabled() || ticket.0 != self.verification_generation {
            return;
        }
        if accepted && self.state.phase == UpdatePhase::Downloading {
            if self.state.last_checked_at.is_none() {
                self.state.last_checked_at = Some(checked_at.to_owned());
            }
            self.patch_current(
                UpdatePhase::Ready,
                self.state.available_version.clone(),
                Some(100.0),
            );
        } else if !accepted {
            self.refuse(self.state.available_version.clone(), checked_at);
        }
    }

    pub fn downloaded_without_verification(&mut self, version: Option<&str>, checked_at: &str) {
        if !self.enabled() {
            return;
        }
        let version = safe_version(version)
            .map(str::to_owned)
            .or_else(|| self.state.available_version.clone());
        if self.state.phase == UpdatePhase::Ready && version == self.state.available_version {
            return;
        }
        self.verification_generation = self.verification_generation.wrapping_add(1);
        if version
            .as_deref()
            .is_none_or(|candidate| !is_newer_version(candidate, &self.state.current_version))
        {
            self.refuse(version, checked_at);
            return;
        }
        if self.state.last_checked_at.is_none() {
            self.state.last_checked_at = Some(checked_at.to_owned());
        }
        self.patch_current(UpdatePhase::Ready, version, Some(100.0));
    }

    #[must_use]
    pub fn request_install(&mut self) -> bool {
        if self.state.phase != UpdatePhase::Ready || !self.state.can_restart {
            return false;
        }
        self.patch_current(
            UpdatePhase::Installing,
            self.state.available_version.clone(),
            self.state.progress,
        );
        true
    }

    pub fn refuse_install(&mut self, checked_at: &str) {
        self.refuse(self.state.available_version.clone(), checked_at);
    }

    /// A release the public feed offered failed its signature or digest.
    pub fn refuse_unverified(&mut self, checked_at: &str) {
        self.refuse(self.state.available_version.clone(), checked_at);
    }

    pub fn error(&mut self, checked_at: &str) {
        if !self.enabled() {
            return;
        }
        let failed_phase = self.state.phase;
        self.state.last_checked_at = Some(checked_at.to_owned());
        self.patch_current(
            UpdatePhase::Error,
            self.state.available_version.clone(),
            None,
        );
        self.state.error = Some(safe_update_error(failed_phase).to_owned());
    }

    fn enabled(&self) -> bool {
        self.packaged && self.feed.is_some()
    }

    fn refuse(&mut self, version: Option<String>, checked_at: &str) {
        if !self.enabled() {
            return;
        }
        self.state.last_checked_at = Some(checked_at.to_owned());
        self.patch_current(UpdatePhase::Error, version, None);
        self.state.error = Some(
            REFUSED_UPDATE_MESSAGE
                .chars()
                .take(MAX_UPDATE_ERROR_LENGTH)
                .collect(),
        );
    }

    fn patch(
        &mut self,
        phase_source: (UpdatePhase, UpdateSource),
        version: Option<String>,
        progress: Option<f64>,
        error: Option<String>,
    ) {
        self.state.phase = phase_source.0;
        self.state.source = phase_source.1;
        self.state.available_version = version;
        self.state.progress = progress;
        self.state.error = error;
        self.refresh_actions();
    }

    fn patch_current(
        &mut self,
        phase: UpdatePhase,
        version: Option<String>,
        progress: Option<f64>,
    ) {
        self.state.phase = phase;
        self.state.available_version = version;
        self.state.progress = progress;
        self.state.error = None;
        self.refresh_actions();
    }

    fn refresh_actions(&mut self) {
        self.state.can_check = self.enabled()
            && matches!(
                self.state.phase,
                UpdatePhase::Idle | UpdatePhase::UpToDate | UpdatePhase::Error | UpdatePhase::Ready
            );
        self.state.can_restart = self.state.phase == UpdatePhase::Ready;
        debug_assert!(self.state.is_valid());
    }
}

#[must_use]
pub const fn safe_update_error(phase: UpdatePhase) -> &'static str {
    if matches!(phase, UpdatePhase::Installing) {
        "macOS could not finish preparing the update. Check again and retry."
    } else {
        "Interface could not complete the update. Check your connection and try again."
    }
}

fn safe_version(version: Option<&str>) -> Option<&str> {
    version.filter(|version| {
        !version.is_empty()
            && version.len() <= MAX_UPDATE_VERSION_LENGTH
            && !version.chars().any(char::is_control)
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedVersion {
    core: (u64, u64, u64),
    prerelease: Option<Vec<String>>,
}

fn parse_version(raw: &str) -> Option<ParsedVersion> {
    let trimmed = raw.trim();
    let without_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = without_v
        .split_once('+')
        .map_or(without_v, |(head, _)| head);
    let (core_text, prerelease_text) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };
    let mut parts = core_text.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let prerelease = match prerelease_text {
        None => None,
        Some("") => return None,
        Some(pre) => {
            let mut identifiers = Vec::new();
            for item in pre.split('.') {
                if item.is_empty()
                    || !item
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                {
                    return None;
                }
                identifiers.push(item.to_owned());
            }
            Some(identifiers)
        }
    };
    Some(ParsedVersion {
        core: (major, minor, patch),
        prerelease,
    })
}

fn compare_prerelease(left: &[String], right: &[String]) -> std::cmp::Ordering {
    for (a, b) in left.iter().zip(right.iter()) {
        if a == b {
            continue;
        }
        let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(na), Ok(nb)) => na.cmp(&nb),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            (Err(_), Err(_)) => a.cmp(b),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    left.len().cmp(&right.len())
}

/// A release version written the one way the installer's feed writes it:
/// no `v`, no build suffix, no surrounding space.
#[must_use]
pub(crate) fn is_plain_version(version: &str) -> bool {
    version.len() <= MAX_UPDATE_VERSION_LENGTH
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        && version.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && parse_version(version).is_some()
}

#[must_use]
pub fn is_newer_version(candidate: &str, current: &str) -> bool {
    let (Some(candidate), Some(current)) = (parse_version(candidate), parse_version(current))
    else {
        return false;
    };
    match candidate.core.cmp(&current.core) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (&candidate.prerelease, &current.prerelease) {
            (None, Some(_)) => true,
            (Some(_) | None, None) => false,
            (Some(cand_pre), Some(curr_pre)) => {
                compare_prerelease(cand_pre, curr_pre) == std::cmp::Ordering::Greater
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::updater::UpdateSource;

    const AT: &str = "2026-08-27T12:00:00.000Z";

    fn machine(packaged: bool) -> UpdateMachine {
        let mut machine = UpdateMachine::new("0.2.0", packaged);
        machine.set_feed(Some(
            NormalizedFeed::new(
                "https://updates.example.test/interface/",
                UpdateSource::Remote,
            )
            .expect("feed"),
        ));
        machine
    }

    #[test]
    fn anti_rollback_rejects_downgrade_same_and_unparseable_versions() {
        assert!(is_newer_version("0.2.1", "0.2.0"));
        assert!(is_newer_version("0.2.0", "0.2.0-rc.1"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
        assert!(!is_newer_version("0.1.9", "0.2.0"));
        assert!(!is_newer_version("0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer_version("not-a-version", "0.2.0"));

        let mut machine = machine(true);
        machine.available(Some("0.1.9"), AT);
        assert_eq!(machine.state().phase, UpdatePhase::UpToDate);
        assert_eq!(
            machine.downloaded_requiring_verification(Some("0.2.0"), true, AT),
            None
        );
        assert_eq!(machine.state().phase, UpdatePhase::Error);
        assert!(!machine.state().can_restart);
    }

    #[test]
    fn development_is_inert_even_with_a_valid_feed() {
        let mut machine = machine(false);
        assert_eq!(machine.state().phase, UpdatePhase::Disabled);
        assert_eq!(machine.state().source, UpdateSource::Remote);
        machine.checking();
        machine.downloaded_without_verification(Some("0.3.0"), AT);
        assert_eq!(machine.state().phase, UpdatePhase::Disabled);
    }

    #[test]
    fn signature_gate_is_fail_closed_and_generation_fenced() {
        let mut machine = machine(true);
        let stale = machine
            .downloaded_requiring_verification(Some("0.3.0"), true, AT)
            .expect("verification");
        let current = machine
            .downloaded_requiring_verification(Some("0.4.0"), true, AT)
            .expect("verification");
        machine.complete_verification(stale, true, AT);
        assert_eq!(machine.state().phase, UpdatePhase::Downloading);
        machine.complete_verification(current, false, AT);
        assert_eq!(machine.state().phase, UpdatePhase::Error);
        assert_eq!(machine.state().available_version.as_deref(), Some("0.4.0"));
        assert_eq!(
            machine.state().error.as_deref(),
            Some(REFUSED_UPDATE_MESSAGE)
        );
        assert!(!machine.request_install());
    }

    #[test]
    fn ready_is_sticky_for_same_download_and_install_is_explicit() {
        let mut machine = machine(true);
        machine.available(Some("0.3.0"), AT);
        machine.downloaded_without_verification(Some("0.3.0"), AT);
        assert_eq!(machine.state().phase, UpdatePhase::Ready);
        machine.checking();
        machine.not_available(AT);
        assert_eq!(machine.state().phase, UpdatePhase::Ready);
        machine.downloaded_without_verification(Some("0.3.0"), AT);
        assert!(machine.request_install());
        assert_eq!(machine.state().phase, UpdatePhase::Installing);
        assert!(!machine.state().can_restart);
    }

    #[test]
    fn progress_is_clamped_and_missing_artifact_is_refused() {
        let mut machine = machine(true);
        machine.progress(Some(-12.0));
        assert_eq!(machine.state().progress, Some(0.0));
        machine.progress(Some(112.0));
        assert_eq!(machine.state().progress, Some(100.0));
        assert_eq!(
            machine.downloaded_requiring_verification(Some("0.3.0"), false, AT),
            None
        );
        assert_eq!(machine.state().phase, UpdatePhase::Error);
        assert!(!machine.state().can_restart);
    }
}
