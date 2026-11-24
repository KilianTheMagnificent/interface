use serde::{Deserialize, Serialize};

use crate::sanitize::for_display;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CliExitCodes {
    pub success: u8,
    pub generic: u8,
    pub validation: u8,
    pub unsent: u8,
    pub unknown: u8,
    pub provider: u8,
    pub denial: u8,
    pub timeout: u8,
}

pub const CLI_EXIT: CliExitCodes = CliExitCodes {
    success: 0,
    generic: 1,
    validation: 2,
    unsent: 3,
    unknown: 4,
    provider: 5,
    denial: 6,
    timeout: 7,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryOutcome {
    DefinitelyUnsent,
    AdapterAccepted,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

impl CommandResult {
    #[must_use]
    pub const fn success(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            exit_code: CLI_EXIT.success,
        }
    }

    #[must_use]
    pub const fn exit(exit_code: u8) -> Self {
        Self {
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
        }
    }

    #[must_use]
    pub fn error(message: impl AsRef<str>, exit_code: u8) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("iface: {}\n", for_display(message.as_ref())),
            exit_code,
        }
    }
}

#[must_use]
pub const fn exit_code_for_send(outcome: DeliveryOutcome) -> u8 {
    match outcome {
        DeliveryOutcome::DefinitelyUnsent => CLI_EXIT.unsent,
        DeliveryOutcome::AdapterAccepted => CLI_EXIT.success,
        DeliveryOutcome::OutcomeUnknown => CLI_EXIT.unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_certainty_has_distinct_scriptable_exit_codes() {
        assert_eq!(exit_code_for_send(DeliveryOutcome::AdapterAccepted), 0);
        assert_eq!(exit_code_for_send(DeliveryOutcome::DefinitelyUnsent), 3);
        assert_eq!(exit_code_for_send(DeliveryOutcome::OutcomeUnknown), 4);
    }

    #[test]
    fn every_public_exit_code_is_stable_and_distinct() {
        let values = [
            CLI_EXIT.success,
            CLI_EXIT.generic,
            CLI_EXIT.validation,
            CLI_EXIT.unsent,
            CLI_EXIT.unknown,
            CLI_EXIT.provider,
            CLI_EXIT.denial,
            CLI_EXIT.timeout,
        ];
        let mut deduplicated = values.to_vec();
        deduplicated.sort_unstable();
        deduplicated.dedup();
        assert_eq!(deduplicated.len(), values.len());
    }
}
