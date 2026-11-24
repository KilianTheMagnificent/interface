//! Project reads that iCloud can hold up.
//!
//! A file iCloud Drive has evicted (Optimize Mac Storage) is *dataless*: its
//! name and size are on disk, its contents are not, and the first `open` or
//! `read` waits, with no deadline of its own, until the file provider has
//! downloaded it. A `stat` never waits. A project read that can meet such a
//! file runs through [`within`]: the read gets a thread of its own and the
//! caller waits for it at most `deadline`. Past that the caller gets
//! [`ReadStalled`] and says [`STILL_DOWNLOADING`] (or its own sentence), and
//! the read is left to finish by itself: a blocked `open` cannot be cancelled
//! from user space, so the thread is abandoned, never joined. Whatever it
//! finally reads is dropped.

use std::{
    sync::mpsc::{RecvTimeoutError, sync_channel},
    time::Duration,
};

/// What a read that ran out of time tells the owner.
pub const STILL_DOWNLOADING: &str =
    "This file is still downloading from iCloud. Try again once it has downloaded.";

/// The read did not finish within its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadStalled;

impl std::fmt::Display for ReadStalled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(STILL_DOWNLOADING)
    }
}

impl std::error::Error for ReadStalled {}

/// Runs `read` on its own thread and waits for it at most `deadline`.
///
/// A panic in `read` is resumed on the caller. If no thread can be started,
/// `read` does not run and the answer is [`ReadStalled`].
///
/// # Errors
///
/// [`ReadStalled`] when `read` has not answered by `deadline`.
pub fn within<T: Send + 'static>(
    deadline: Duration,
    read: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ReadStalled> {
    let (sender, receiver) = sync_channel(1);
    let started = std::thread::Builder::new()
        .name("interface-bounded-read".to_owned())
        .spawn(move || {
            let _ = sender.send(read());
        });
    // No thread to run it on: the read cannot be bounded, so it is not
    // started either; the caller says the file is unavailable for now.
    let Ok(handle) = started else {
        return Err(ReadStalled);
    };
    match receiver.recv_timeout(deadline) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(ReadStalled),
        Err(RecvTimeoutError::Disconnected) => match handle.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => Err(ReadStalled),
        },
    }
}

/// Whether a file's contents are off the Mac: opening or reading it waits for
/// a download, while its `stat` (size included) is already true.
#[cfg(target_os = "macos")]
#[must_use]
pub fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    /// `<sys/stat.h>`: the file's contents are not on local storage.
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

/// Whether a file's contents are off the Mac. Only macOS has dataless files.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub const fn is_dataless(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs::File,
        io::Read,
        process::Command,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn a_read_that_blocks_on_open_fails_at_its_deadline_and_a_quick_one_answers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        // A FIFO with no writer blocks `open` forever, as a dataless iCloud
        // file blocks until the file provider has downloaded it.
        let fifo = directory.path().join("evicted.txt");
        assert!(
            Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        let started = Instant::now();
        let blocked = fifo.clone();
        let answer = within(Duration::from_millis(200), move || {
            let mut text = String::new();
            File::open(&blocked).and_then(|mut file| file.read_to_string(&mut text))
        });
        assert_eq!(answer.err(), Some(ReadStalled));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(ReadStalled.to_string(), STILL_DOWNLOADING);

        let local = directory.path().join("local.txt");
        std::fs::write(&local, "here").expect("local file");
        assert_eq!(
            within(Duration::from_secs(5), move || std::fs::read_to_string(
                local
            ))
            .expect("a local read answers")
            .expect("read"),
            "here"
        );
        let metadata = std::fs::metadata(directory.path().join("local.txt")).expect("stat");
        assert!(!is_dataless(&metadata), "a file on disk is not dataless");
        // Release the abandoned reader so the test leaves no thread behind.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
    }

    #[test]
    #[should_panic(expected = "read failed loudly")]
    fn a_panic_in_the_read_reaches_the_caller() {
        let _ = within(Duration::from_secs(5), || -> u8 {
            panic!("read failed loudly")
        });
    }
}
