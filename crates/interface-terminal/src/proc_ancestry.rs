//! Which live pane a process belongs to, read off the kernel's parent chain.
//!
//! The pane MCP socket is one fixed path every pane's CLI connects to through
//! `nc`, so the only thing that says which pane is calling is the connecting
//! process's ancestry: `nc` → the CLI → (a shell) → the PTY child the service
//! spawned. One `/bin/ps` listing is parsed into a pid→ppid map and walked
//! upward until a pid the service owns appears. Same shell-out shape as
//! `proc_cwd.rs`; the workspace forbids `unsafe`, so no `proc_pidinfo`.
//! [`ProcessListing`] is the same walk with each process's start time, for
//! the host's team tools socket, which must also tell a provider process
//! from a later one that reused its pid.

use std::collections::HashMap;

/// Deep enough for any CLI's spawn tree (Claude runs MCP servers two or three
/// forks down); short enough that a corrupt listing cannot spin.
const MAX_HOPS: usize = 32;

/// pid → parent pid for every process `ps` can see. `None` when the listing
/// could not be read; a caller then refuses rather than guesses.
#[cfg(unix)]
pub(crate) fn process_parents() -> Option<HashMap<u32, u32>> {
    Some(
        process_table()?
            .into_iter()
            .map(|(pid, (parent, _))| (pid, parent))
            .collect(),
    )
}

#[cfg(not(unix))]
pub(crate) fn process_parents() -> Option<HashMap<u32, u32>> {
    None
}

/// pid → (parent pid, process group id) for every process `ps` can see, from
/// one listing. Workspace sleep uses the group ids to find each group leader
/// without a `ps -p` per descendant; caller identity reads the parents.
#[cfg(unix)]
pub(crate) fn process_table() -> Option<HashMap<u32, (u32, u32)>> {
    // `ps` reads kernel process tables only, never a mount, so unlike the
    // `lsof` call in `proc_cwd.rs` it needs no deadline.
    let output = crate::PsProbe {
        columns: &["pid", "ppid", "pgid"],
        select: &["-ax"],
        utc: false,
        stderr: false,
    }
    .output()
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_table(std::str::from_utf8(&output.stdout).ok()?))
}

#[cfg(unix)]
fn parse_table(listing: &str) -> HashMap<u32, (u32, u32)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let group = fields.next()?.parse().ok()?;
            Some((pid, (parent, group)))
        })
        .collect()
}

/// One `ps` listing of every process's parent and start time, for a caller
/// that must also tell a process from a later one that reused its pid (the
/// host's team tools socket asks whether a caller descends from one exact
/// provider process). Start times are `lstart` in the C locale and UTC,
/// single-spaced, as the pane capture receipts record them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessListing {
    parents: HashMap<u32, u32>,
    starts: HashMap<u32, String>,
}

impl ProcessListing {
    /// Every process `ps` can see now. `None` when the listing could not be
    /// read; a caller then refuses rather than guesses.
    #[cfg(unix)]
    #[must_use]
    pub fn read() -> Option<Self> {
        let output = crate::PsProbe {
            columns: &["pid", "ppid", "lstart"],
            select: &["-ax"],
            utc: true,
            stderr: false,
        }
        .output()
        .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(std::str::from_utf8(&output.stdout).ok()?.lines().collect())
    }

    #[cfg(not(unix))]
    #[must_use]
    pub fn read() -> Option<Self> {
        None
    }

    /// When `pid` started, if it is in the listing.
    #[must_use]
    pub fn start(&self, pid: u32) -> Option<&str> {
        self.starts.get(&pid).map(String::as_str)
    }

    /// The first process at or above `pid` for which `owned` answers true:
    /// the pane tools' bounded walk ([`owned_ancestor`]) over this listing.
    #[must_use]
    pub fn owned_ancestor(&self, pid: u32, owned: impl Fn(u32) -> bool) -> Option<u32> {
        owned_ancestor(pid, &self.parents, owned)
    }
}

/// One `pid ppid lstart…` line per process, as [`ProcessListing::read`]
/// asks `ps` for them; a line without all three is skipped.
impl<'a> FromIterator<&'a str> for ProcessListing {
    fn from_iter<I: IntoIterator<Item = &'a str>>(lines: I) -> Self {
        let mut listing = Self::default();
        for line in lines {
            let mut fields = line.split_whitespace();
            let (Some(pid), Some(parent)) = (
                fields.next().and_then(|field| field.parse::<u32>().ok()),
                fields.next().and_then(|field| field.parse::<u32>().ok()),
            ) else {
                continue;
            };
            let start = fields.collect::<Vec<_>>().join(" ");
            if start.is_empty() {
                continue;
            }
            listing.parents.insert(pid, parent);
            listing.starts.insert(pid, start);
        }
        listing
    }
}

/// The first process at or above `pid` for which `owned` answers true, or
/// `None` once the chain ends, repeats, or runs past `MAX_HOPS`.
pub(crate) fn owned_ancestor(
    pid: u32,
    parents: &HashMap<u32, u32>,
    owned: impl Fn(u32) -> bool,
) -> Option<u32> {
    let mut current = pid;
    for _ in 0..MAX_HOPS {
        if owned(current) {
            return Some(current);
        }
        let parent = *parents.get(&current)?;
        // pid 1 is its own parent on some kernels and 0's parent is 0; either
        // way the walk has reached the top.
        if parent == current || parent == 0 {
            return None;
        }
        current = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{owned_ancestor, process_parents};

    #[test]
    fn walks_up_to_the_first_owned_pid_and_stops_at_the_top_or_a_cycle() {
        let parents = HashMap::from([
            (1, 0),
            (10, 1),
            (20, 10),
            (30, 20),
            (40, 40),
            (50, 60),
            (60, 50),
        ]);
        assert_eq!(owned_ancestor(30, &parents, |pid| pid == 10), Some(10));
        assert_eq!(owned_ancestor(10, &parents, |pid| pid == 10), Some(10));
        assert_eq!(owned_ancestor(30, &parents, |pid| pid == 99), None);
        assert_eq!(owned_ancestor(77, &parents, |_| true), Some(77));
        assert_eq!(owned_ancestor(77, &parents, |_| false), None);
        assert_eq!(owned_ancestor(40, &parents, |pid| pid == 1), None);
        assert_eq!(owned_ancestor(50, &parents, |pid| pid == 1), None);
        let chain: HashMap<u32, u32> = (1..=40).map(|pid| (pid + 1, pid)).collect();
        let long = chain;
        assert_eq!(owned_ancestor(41, &long, |pid| pid == 1), None);
        assert_eq!(owned_ancestor(41, &long, |pid| pid == 20), Some(20));
    }

    #[cfg(unix)]
    #[test]
    fn finds_this_test_process_above_a_child_it_spawned() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let parents = process_parents().expect("process listing");
        let me = std::process::id();
        assert_eq!(
            owned_ancestor(child.id(), &parents, |pid| pid == me),
            Some(me)
        );
        assert_eq!(
            owned_ancestor(child.id(), &parents, |pid| pid == u32::MAX - 7),
            None
        );
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn a_listing_names_each_parent_and_start_and_walks_like_the_pane_tools() {
        let listing: super::ProcessListing = [
            "  1     0 Mon Nov 23 09:00:00 2026",
            " 10     1 Mon Nov 23 10:00:00 2026",
            " 20    10 Mon Nov  3 10:00:01 2026",
            "garbage",
            " 30    20",
        ]
        .into_iter()
        .collect();
        assert_eq!(listing.start(10), Some("Mon Nov 23 10:00:00 2026"));
        assert_eq!(listing.start(20), Some("Mon Nov 3 10:00:01 2026"));
        assert_eq!(listing.start(30), None, "a line without a start is skipped");
        assert_eq!(listing.owned_ancestor(20, |pid| pid == 1), Some(1));
        assert_eq!(listing.owned_ancestor(30, |pid| pid == 1), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_listing_finds_this_test_process_above_a_child_it_spawned() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let listing = super::ProcessListing::read().expect("process listing");
        let me = std::process::id();
        assert_eq!(
            listing.owned_ancestor(child.id(), |pid| pid == me),
            Some(me)
        );
        assert!(listing.start(me).is_some_and(|start| !start.is_empty()));
        assert!(listing.start(child.id()).is_some());
        child.kill().ok();
        child.wait().ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_process_table_names_each_parent_and_group() {
        assert_eq!(
            super::parse_table("  1 0 1\n 10 1 10\n 20 10 10\ngarbage\n 30 20\n"),
            HashMap::from([(1, (0, 1)), (10, (1, 10)), (20, (10, 10))])
        );
        let table = super::process_table().expect("process listing");
        let me = std::process::id();
        let parents = process_parents().expect("process listing");
        assert_eq!(
            table.get(&me).map(|(parent, _)| *parent),
            parents.get(&me).copied()
        );
    }
}
