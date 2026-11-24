//! The service's one 1 Hz ticker. Rather than a sleeping thread per pane per
//! task, a single thread runs every pane's workspace-sleep policy and the
//! shell-display and Grok-title refreshes armed for it, and exits once the
//! service holds no terminal entries; the next create or restart starts it
//! again.
//!
//! Every `WARM_TICKS` it also re-fetches running panes' startup files; see
//! `startup_download`.
//!
//! Each pane's ticks keep the start gate and permanent stop of the thread
//! they replace: a tick is armed once, for the generation created or
//! restarted, and cleared for good when it reports it is done. Slow ticks
//! (a foreground `ps`, a suspension's process scan, a session file read)
//! run on a small bounded set of workers, so one slow pane cannot hold up
//! every other pane's tick, and a panic ends only that pane's tick.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{
    Arc, Duration, Mutex, Shared, TerminalGeneration, TerminalId, TerminalLaunchProfileId,
    TerminalProcessControl, TerminalService, TerminalState, TerminalSummary, Weak, lock, thread,
};

const TICK: Duration = Duration::from_secs(1);
const TICK_WORKERS: usize = 4;
/// How long a second's ticks run on the ticker thread before the rest spread
/// over `TICK_WORKERS` threads.
const INLINE_TICK_BUDGET: Duration = Duration::from_millis(20);
/// Five minutes: often enough that a re-evicted HEAD or CLAUDE.md is local
/// again before the next pane starts, rare enough to cost nothing.
const WARM_TICKS: u32 = 300;

/// The per-generation refreshes the ticker runs for one entry.
#[derive(Clone, Copy, Default)]
pub(super) struct EntryTicks {
    pub(super) shell_display: bool,
    pub(super) grok_title: bool,
    pub(super) startup_download: bool,
}

impl EntryTicks {
    /// The refreshes a generation gets when it is created or restarted: its
    /// startup files always, the shell display while it runs a shell, and
    /// the Grok title only when it was launched as Grok with a minted session
    /// id (a shell whose foreground later becomes Grok is never picked up).
    pub(super) fn armed(summary: &TerminalSummary, shell_display: bool) -> Self {
        Self {
            shell_display,
            grok_title: summary
                .launch
                .is_some_and(|launch| launch.profile_id == TerminalLaunchProfileId::Grok)
                && summary
                    .launch_session
                    .as_ref()
                    .is_some_and(|session| session.id.is_some()),
            startup_download: true,
        }
    }

    const fn any(self) -> bool {
        self.shell_display || self.grok_title || self.startup_download
    }
}

type SleepTick = (
    Arc<Mutex<super::workspace_sleep::WorkspacePower>>,
    Arc<dyn TerminalProcessControl>,
    TerminalLaunchProfileId,
);

struct DueTick {
    id: TerminalId,
    generation: TerminalGeneration,
    sleep: Option<SleepTick>,
    ticks: EntryTicks,
}

impl TerminalService {
    /// Starts the ticker thread. The caller set `ServiceState::ticker` in the
    /// same state-lock section that inserted an entry, so a ticker that is
    /// about to exit cannot strand a new terminal.
    pub(super) fn spawn_ticker(&self) {
        let shared = Arc::downgrade(&self.shared);
        thread::spawn(move || run_ticker(&shared));
    }
}

fn run_ticker(shared: &Weak<Shared>) {
    let mut ticks: u32 = 0;
    loop {
        thread::sleep(TICK);
        let Some(strong) = shared.upgrade() else {
            return;
        };
        ticks = ticks.wrapping_add(1);
        if ticks % WARM_TICKS == 0 {
            super::startup_download::warm_startup_files(&strong);
        }
        let due: Vec<DueTick> = {
            let mut state = lock(&strong.state);
            if state.entries.is_empty() {
                state.ticker = false;
                return;
            }
            state
                .entries
                .values()
                .filter_map(|entry| {
                    let sleep = (!entry.closing && entry.summary.state == TerminalState::Running)
                        .then(|| {
                            (
                                Arc::clone(&entry.power),
                                Arc::clone(&entry.resources.control),
                                entry
                                    .summary
                                    .launch
                                    .map_or(TerminalLaunchProfileId::Shell, |launch| {
                                        launch.profile_id
                                    }),
                            )
                        });
                    (sleep.is_some() || entry.ticks.any()).then(|| DueTick {
                        id: entry.summary.id,
                        generation: entry.summary.generation,
                        sleep,
                        ticks: entry.ticks,
                    })
                })
                .collect()
        };
        drop(strong);
        let finished = run_bounded(&due, |tick| {
            let done = tick_entry(tick, shared);
            done.any().then_some((tick.id, tick.generation, done))
        });
        if finished.is_empty() {
            continue;
        }
        let Some(strong) = shared.upgrade() else {
            return;
        };
        let mut state = lock(&strong.state);
        for (id, generation, done) in finished {
            if let Some(entry) = state
                .entries
                .get_mut(&id)
                .filter(|entry| entry.summary.generation == generation)
            {
                entry.ticks.shell_display &= !done.shell_display;
                entry.ticks.grok_title &= !done.grok_title;
                entry.ticks.startup_download &= !done.startup_download;
            }
        }
    }
}

/// Runs `tick` for every due entry and returns what the ticks reported.
///
/// Almost every tick is a lock and a return, so the entries run on this
/// thread first; only once a second's ticks outlast `INLINE_TICK_BUDGET` (a
/// `ps` scan, a suspension) do the rest spread over up to `TICK_WORKERS`
/// threads. A quiet second creates no thread.
fn run_bounded<T: Sync, R: Send>(due: &[T], tick: impl Fn(&T) -> Option<R> + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let finished = Mutex::new(Vec::new());
    let run_one = |entry: &T| {
        if let Some(report) = tick(entry) {
            lock(&finished).push(report);
        }
    };
    let started = std::time::Instant::now();
    while started.elapsed() < INLINE_TICK_BUDGET {
        let Some(entry) = due.get(next.fetch_add(1, Ordering::Relaxed)) else {
            return finished
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        };
        run_one(entry);
    }
    let work = || {
        while let Some(entry) = due.get(next.fetch_add(1, Ordering::Relaxed)) {
            run_one(entry);
        }
    };
    let remaining = due.len().saturating_sub(next.load(Ordering::Relaxed));
    let workers = remaining.min(TICK_WORKERS);
    if workers <= 1 {
        work();
    } else {
        thread::scope(|scope| {
            for _ in 1..workers {
                scope.spawn(work);
            }
            work();
        });
    }
    finished
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One second of one entry: its sleep policy, then its armed refreshes. A
/// panicking refresh ends for good, as its dedicated thread used to.
fn tick_entry(tick: &DueTick, shared: &Weak<Shared>) -> EntryTicks {
    if let Some((power, control, profile)) = &tick.sleep {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            super::workspace_sleep::tick_workspace_sleep(power, control.as_ref(), *profile);
        }));
    }
    let run = |refresh: fn(&Weak<Shared>, TerminalId, TerminalGeneration) -> bool| {
        !catch_unwind(AssertUnwindSafe(|| {
            refresh(shared, tick.id, tick.generation)
        }))
        .unwrap_or(false)
    };
    EntryTicks {
        shell_display: tick.ticks.shell_display
            && run(super::provider_recovery::refresh_shell_display),
        grok_title: tick.ticks.grok_title && run(super::titles::poll_grok_title),
        startup_download: tick.ticks.startup_download
            && run(super::startup_download::refresh_startup_download),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn slow_ticks_share_bounded_workers_and_every_entry_still_ticks() {
        let due: Vec<u32> = (0..8).collect();
        let started = Instant::now();
        let mut reports = run_bounded(&due, |entry| {
            thread::sleep(Duration::from_millis(100));
            (entry % 2 == 0).then_some(*entry)
        });
        reports.sort_unstable();
        assert_eq!(reports, [0, 2, 4, 6]);
        // Eight 100 ms ticks on four workers take about 200 ms, not 800 ms.
        assert!(started.elapsed() < Duration::from_millis(600));
    }
}
