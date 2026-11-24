use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod artifact_receipt;
mod check_run;
mod closeout;
mod dependencies;
mod dev_restart;
mod fsx;
mod gate;
mod gate_cache;
mod landings;
mod navigation;
mod package;
mod pins;
mod pipeline_state;
mod pipeline_status;
mod promote;
mod promotion_run;
mod prove;
mod provider_canary;
mod release;
#[cfg(test)]
mod test_fixtures;
mod test_runner;
mod update_channel;
mod update_log;
mod worktree_prune;

/// The workspace gate, the macOS release path, and the AGENTS.md closeout,
/// owned by Rust.
#[derive(Debug, Parser)]
#[command(name = "cargo xtask")]
struct Arguments {
    #[command(subcommand)]
    command: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// Find task owners, entry points, rules and focused checks without reading source.
    Navigate {
        /// Case-insensitive terms, route id, or repository path. Quote multiple words.
        query: Option<String>,
        #[arg(long)]
        json: bool,
        /// Validate the map's paths and Cargo workspace coverage; runs no task checks.
        #[arg(long, conflicts_with = "query")]
        check: bool,
    },
    /// Run the deterministic workspace gate. With no flags or --full, both
    /// suites run. --changed scopes the checks and reuses matching source and
    /// toolchain receipts, including checks earned before committing.
    Check {
        /// Run only the halves of the gate the diff since --base can affect.
        #[arg(long)]
        changed: bool,
        /// Revision --changed compares against. Defaults to origin/dev.
        #[arg(long, default_value = "origin/dev")]
        base: String,
        /// Force the full gate even with --changed.
        #[arg(long)]
        full: bool,
    },
    /// Build, sign, inspect and smoke a clean committed source tree.
    /// --trust-gate reuses matching source/toolchain receipts for both suites.
    PackageMac {
        /// Launch the exact app extracted from the verified ZIP and probe its renderer/host bridge.
        #[arg(long)]
        smoke: bool,
        /// Permit deliberate replacement of an existing equal-version local artifact set.
        #[arg(long)]
        allow_same_version: bool,
        /// Reuse checks proven against identical source content and toolchains.
        #[arg(long)]
        trust_gate: bool,
    },
    /// Prepare, verify and publish a pinned approved revision or named landings.
    /// Preparation stays in the managed worktree. A durable receipt resumes
    /// checks, verified packages, ref publication and feed staging independently.
    /// Main and integrated dev publish atomically; installation stays an owner action.
    Promote {
        /// New Interface version; must be newer than package.json and the feed.
        #[arg(long)]
        version: String,
        /// Ship only this landing (name substring or SHA prefix); repeatable.
        /// Without it, all of dev is promoted.
        #[arg(long = "change")]
        changes: Vec<String>,
        /// Exact approved dev revision; preparation never changes the caller's checkout.
        #[arg(long, conflicts_with = "changes")]
        revision: Option<String>,
        /// Acknowledge leaving this user-facing landing on Dev (name
        /// substring or SHA prefix); repeatable. Promote refuses to leave one
        /// out silently.
        #[arg(long = "leave-out")]
        leave_out: Vec<String>,
        /// Check the preconditions and print the plan, changing nothing.
        #[arg(long)]
        dry_run: bool,
        /// Stop after packaging: no push of dev or main, and no feed staged.
        #[arg(long)]
        no_push: bool,
        /// Managed release checkout. Defaults to ~/.interface-release.
        #[arg(long)]
        release_worktree: Option<PathBuf>,
    },
    /// Re-inspect an existing private Electron Builder output without publishing it.
    VerifyPackage {
        /// Electron Builder output containing latest-mac.yml, app, DMG, and ZIP.
        #[arg(long)]
        source: PathBuf,
        /// Launch the exact app extracted from the verified ZIP.
        #[arg(long)]
        smoke: bool,
    },
    /// Validate and atomically replace an Electron updater feed.
    StageUpdate {
        /// Source directory containing latest-mac.yml and its referenced artifacts.
        #[arg(long)]
        source: PathBuf,
        /// Dedicated feed directory. Defaults to Interface's owner-private local feed.
        #[arg(long)]
        target: Option<PathBuf>,
        /// Permit deliberate replacement of an existing equal-version local feed.
        #[arg(long)]
        allow_same_version: bool,
    },
    /// Make a signing key for the public update channel. Its private half
    /// stays in ~/.config/interface-release on this Mac.
    UpdateKey {
        /// Which key this is: primary, backup, or any lowercase name.
        #[arg(long, default_value = "primary")]
        name: String,
        /// List the new public key in crates/interface-updater/release-channel.json.
        #[arg(long)]
        add_to_channel: bool,
    },
    /// Sign the release staged in the local feed for the public update
    /// channel, rehearse it as an installed app would take it, and write the
    /// feed folder. With --github, upload it and check it live.
    PublishUpdate {
        /// Folder holding latest-mac.yml and its ZIP. Defaults to the local feed.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Where to write the feed folder. Defaults to release/public-feed/<version>.
        #[arg(long)]
        out: Option<PathBuf>,
        /// The signing key to use, as named when it was made.
        #[arg(long, default_value = "primary")]
        key: String,
        /// Upload to this GitHub repository's releases: <owner>/<repo>.
        #[arg(long)]
        github: Option<String>,
        /// What the release page says. Defaults to the version.
        #[arg(long)]
        notes: Option<String>,
        /// Publish a build Apple has not notarized (an Apple Development build).
        /// It must still pass the hardened-runtime, timestamp and team checks.
        #[arg(long)]
        allow_unnotarized: bool,
        /// Only read the live feed and say what it offers.
        #[arg(long, conflicts_with_all = ["source", "out", "github", "notes"])]
        check: bool,
        /// With --check, also download the archive and compare it to its signature.
        #[arg(long, requires = "check")]
        download: bool,
    },
    /// Merge origin/dev into this task branch and land it on dev as one named
    /// merge commit.
    ///
    /// The landing's subject is `--change`; without it, closeout refuses
    /// unless the branch adds exactly one non-merge commit, whose subject
    /// then names the landing. `--kind`, `--check` and `--pin` say what the
    /// landing is, what the owner should see, and which tests prove it; a
    /// docs-only landing is `record` without them. `cargo xtask landings`
    /// lists the landings main lacks and `promote --change` ships one of them.
    Closeout {
        /// One line saying what this landing is, for the owner to pick later.
        #[arg(long)]
        change: Option<String>,
        /// feature, fix, look, speed, internal or record.
        #[arg(long)]
        kind: Option<String>,
        /// What the owner should see when it works (required for feature,
        /// fix, look and speed; one line, at most 160 characters).
        #[arg(long)]
        check: Option<String>,
        /// A test proving the landing, `<path>::<test name>`; repeatable.
        /// Feature, fix and look landings need at least one.
        #[arg(long = "pin")]
        pins: Vec<String>,
    },
    /// Show a fix is a fix: run one pinned test on the base revision (in a
    /// kept worktree under ~/.interface-pipeline/prove) and then on this
    /// checkout. Prints one line per side (passed, failed, absent) and exits
    /// 0 only when the base did not pass and this checkout did; 1 when this
    /// checkout fails, 2 when the base already passes, 3 when a side could
    /// not run. Writes no gate receipt and moves no ref.
    Prove {
        /// The test, `<path>::<test name>` as for closeout's --pin.
        #[arg(long)]
        pin: String,
        /// The revision the test must fail on. Defaults to origin/dev (not fetched).
        #[arg(long, default_value = "origin/dev")]
        base: String,
    },
    /// List the landings on dev that main does not carry yet.
    ///
    /// One line per landing, oldest first: short SHA, date, name, task
    /// branch, and which other unshipped landings change a common file. A
    /// landing already shipped by `promote --change` is recognized by the
    /// `cherry picked from` line on main and left out.
    Landings,
    /// Update ~/Developer/.interface-dev and relaunch the Dev app on it.
    DevRestart,
    /// Check each installed provider CLI against what Interface depends on
    /// (version, flags, hook names, capture strings). Runs only `--version`,
    /// `--help`, listing and schema commands and reads the installed files:
    /// never a model turn. Exits non-zero when something no longer matches.
    ProviderCanary {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show landed/running revisions, checks, release progress, timings and the feed.
    Status {
        #[arg(long)]
        json: bool,
        /// Refresh remote-tracking refs before reading status.
        #[arg(long)]
        fetch: bool,
    },
    /// Regenerate or verify docs/UPDATES.md from git release and landing history.
    UpdateLog {
        /// Verify docs/UPDATES.md matches git history without writing.
        #[arg(long)]
        check: bool,
    },
    /// List worktrees merged into origin/dev; --apply removes eligible ones.
    WorktreePrune {
        /// Remove the candidates instead of only listing them.
        #[arg(long)]
        apply: bool,
    },
}

#[allow(clippy::too_many_lines)] // one arm per command
fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let root = workspace_root()?;
    match arguments.command {
        Task::Navigate { query, json, check } => {
            navigation::run(&root, query.as_deref(), json, check)
        }
        Task::Check {
            changed,
            base,
            full,
        } => gate::run_gate(&root, changed, &base, full),
        Task::PackageMac {
            smoke,
            allow_same_version,
            trust_gate,
        } => {
            let result = promotion_run::with_promotion_lock(|| {
                let gate = if trust_gate {
                    package::GateTrust::Receipts
                } else {
                    package::GateTrust::Run
                };
                package::package_mac(&root, allow_same_version, smoke, gate)
            })?;
            println!("Packaged and verified {}.", result.verified);
            println!("Updated {}:", result.publication.release_dir.display());
            for artifact in result.publication.artifacts {
                println!("  {artifact}");
            }
            Ok(())
        }
        Task::Promote {
            version,
            changes,
            revision,
            leave_out,
            dry_run,
            no_push,
            release_worktree,
        } => promotion_run::run(
            &root,
            &promotion_run::Request {
                version,
                changes,
                revision,
                leave_out,
                dry_run,
                no_push,
                worktree: release_worktree,
            },
        ),
        Task::VerifyPackage { source, smoke } => {
            let verified = package::verify_package_directory(&root, &source, smoke)?;
            println!("Verified {verified}.");
            Ok(())
        }
        Task::StageUpdate {
            source,
            target,
            allow_same_version,
        } => {
            let target = target.map_or_else(release::default_feed_target, Ok)?;
            let staged = promotion_run::with_promotion_lock(|| {
                release::stage_update_feed(&source, &target, allow_same_version)
            })?;
            println!(
                "Staged Interface {} update feed at {}",
                staged.version,
                staged.target_dir.display()
            );
            for artifact in staged.artifacts {
                println!("  {artifact}");
            }
            release::nudge_running_production_app();
            Ok(())
        }
        Task::UpdateKey {
            name,
            add_to_channel,
        } => update_channel::new_key(&root, &name, add_to_channel),
        Task::PublishUpdate {
            source,
            out,
            key,
            github,
            notes,
            allow_unnotarized,
            check,
            download,
        } => {
            if check {
                return update_channel::check_live(&root, download);
            }
            update_channel::publish(
                &root,
                &update_channel::PublishRequest {
                    source,
                    out,
                    key,
                    github,
                    notes,
                    allow_unnotarized,
                },
            )
        }
        Task::Closeout {
            change,
            kind,
            check,
            pins,
        } => closeout::closeout(
            &root,
            change.as_deref(),
            &closeout::LandingRequest { kind, check, pins },
        ),
        Task::Prove { pin, base } => {
            let code = prove::prove(&root, &pin, &base).unwrap_or_else(|error| {
                eprintln!("prove: could not run: {error:#}");
                prove::Verdict::CouldNotRun.exit_code()
            });
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Task::Landings => landings::landings_command(&root),
        Task::DevRestart => dev_restart::dev_restart(None),
        Task::ProviderCanary { json } => provider_canary::provider_canary(&root, json),
        Task::Status { json, fetch } => pipeline_status::status(&root, json, fetch),
        Task::UpdateLog { check } => update_log::run(&root, check),
        Task::WorktreePrune { apply } => worktree_prune::worktree_prune(&root, apply),
    }
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .context("xtask must be one directory below the workspace root")
}
