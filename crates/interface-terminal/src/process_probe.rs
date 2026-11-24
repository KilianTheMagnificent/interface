//! The one way Interface runs `/bin/ps`: argv only (never a shell), the C
//! locale, stdin closed. Each caller keeps its own columns, selection,
//! start-time zone, output bound, deadline and failure prose; parsing a row
//! stays with the caller, whose checks differ.

use std::{
    io,
    process::{Command, Output, Stdio},
};

/// One `ps` probe.
#[derive(Clone, Copy, Debug)]
pub struct PsProbe<'a> {
    /// `-o <column>=` for each, in order, so no header line is printed.
    pub columns: &'a [&'a str],
    /// The selection and any other options, such as `["-p", "42"]`,
    /// `["-ww", "-g", "42"]` or `["-ax"]`.
    pub select: &'a [&'a str],
    /// Print start times (`lstart`) in UTC rather than the local zone.
    pub utc: bool,
    /// Keep stderr for the caller's error text; otherwise it is discarded.
    pub stderr: bool,
}

impl PsProbe<'_> {
    /// The command line, for a caller that runs it under its own bound.
    #[must_use]
    pub fn command(&self) -> Command {
        let mut command = Command::new("/bin/ps");
        for column in self.columns {
            command.arg("-o").arg(format!("{column}="));
        }
        command
            .args(self.select)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(if self.stderr {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        if self.utc {
            command.env("TZ", "UTC");
        }
        command
    }

    /// Runs the probe to completion. `ps` reads kernel process tables only,
    /// never a mount, so an unbounded wait is the callers' long-standing
    /// choice wherever they make it.
    ///
    /// # Errors
    ///
    /// `ps` could not be started or waited on.
    pub fn output(&self) -> io::Result<Output> {
        self.command().output()
    }
}

#[cfg(test)]
mod tests {
    use super::PsProbe;

    #[test]
    fn a_probe_names_each_column_without_a_header_and_reads_in_utc_when_asked() {
        let pid = std::process::id().to_string();
        let probe = PsProbe {
            columns: &["pid", "lstart"],
            select: &["-p", &pid],
            utc: true,
            stderr: false,
        };
        let command = probe.command();
        assert_eq!(command.get_program(), "/bin/ps");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-o", "pid=", "-o", "lstart=", "-p", pid.as_str()]
        );
        let envs = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<Vec<_>>();
        assert!(envs.contains(&("LC_ALL".into(), Some("C".into()))));
        assert!(envs.contains(&("TZ".into(), Some("UTC".into()))));

        let output = probe.output().unwrap();
        assert!(output.status.success());
        let row = String::from_utf8(output.stdout).unwrap();
        let mut fields = row.split_whitespace();
        assert_eq!(fields.next(), Some(pid.as_str()));
        assert_eq!(fields.count(), 5, "lstart is five words: {row:?}");
    }
}
