//! `cargo xtask provider-canary`: the provider CLI drift canary on demand.
//!
//! The canary lives in `crates/interface-terminal/src/provider_canary.rs`
//! and runs through `iface doctor providers`; this builds and runs that, so
//! xtask keeps its own small dependency set.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

pub fn provider_canary(root: &Path, json: bool) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(arguments(json))
        .stdin(Stdio::null());
    let status = command
        .status()
        .context("could not run iface doctor providers")?;
    if !status.success() {
        bail!(
            "a provider CLI no longer matches what Interface depends on (or the check could not run); see above"
        );
    }
    Ok(())
}

fn arguments(json: bool) -> Vec<&'static str> {
    let mut arguments = vec![
        "run",
        "--locked",
        "--quiet",
        "--package",
        "iface",
        "--",
        "doctor",
        "providers",
    ];
    if json {
        arguments.extend(["--output", "json"]);
    }
    arguments
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_canary_runs_iface_doctor_providers() {
        assert_eq!(
            super::arguments(false),
            [
                "run",
                "--locked",
                "--quiet",
                "--package",
                "iface",
                "--",
                "doctor",
                "providers"
            ]
        );
        assert_eq!(super::arguments(true)[8..], ["--output", "json"]);
    }
}
