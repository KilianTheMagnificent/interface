//! Tells the desktop when a theme file is added, removed, renamed or saved, so
//! a theme edited in another app repaints Interface as soon as it is saved.
//!
//! A fingerprint of the folder (names, sizes and modification times; the
//! bytes are read only when an appearance read asks for them) polled on the
//! timer task, rather than an OS file watcher: it costs a handful of `stat`
//! calls, follows a folder that is a link into a dotfiles repository, and an
//! editor's burst of writes collapses into a single push.

use std::path::PathBuf;
use std::time::Duration;

use interface_contracts::ipc::CHANNELS;
use interface_desktop_services::CustomThemeLibrary;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::protocol::PushFrame;

/// How often the folder is checked.
#[derive(Clone, Copy, Debug)]
pub struct ThemeFolderPoll {
    /// While the folder holds a theme file: a saved edit repaints this soon.
    pub themes: Duration,
    /// While it holds none, as on almost every Mac: a first theme dropped in
    /// shows within this.
    pub empty: Duration,
}

/// Every second while there are themes to edit, every five while there are none.
pub const THEME_FOLDER_POLL: ThemeFolderPoll = ThemeFolderPoll {
    themes: Duration::from_secs(1),
    empty: Duration::from_secs(5),
};

/// Creates the themes folder when it is missing (so people can find it),
/// then pushes `iface:appearanceChanged` after each change until the push
/// channel closes or the task is aborted.
#[must_use]
pub fn spawn_theme_folder_watcher(
    directory: PathBuf,
    pushes: mpsc::Sender<PushFrame>,
    poll: ThemeFolderPoll,
) -> JoinHandle<()> {
    let library = CustomThemeLibrary::new(directory);
    tokio::spawn(async move {
        // A `mkdir` and then a `readdir` plus a `stat` per theme file each
        // check: cheaper inline than a hop to the blocking pool.
        if let Err(error) = library.ensure_directory() {
            tracing::debug!(%error, "the themes folder could not be created");
        }
        let (mut last, mut themes) = library.scan();
        loop {
            tokio::time::sleep(if themes == 0 { poll.empty } else { poll.themes }).await;
            let (current, count) = library.scan();
            themes = count;
            if current == last {
                continue;
            }
            last = current;
            let Ok(frame) = PushFrame::new(CHANNELS.appearance_changed, serde_json::json!({}))
            else {
                break;
            };
            if pushes.send(frame).await.is_err() {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use tempfile::TempDir;
    use tokio::sync::mpsc;

    use super::{ThemeFolderPoll, spawn_theme_folder_watcher};

    #[tokio::test]
    async fn creates_the_folder_and_pushes_once_per_change() {
        let temp = TempDir::new().unwrap();
        let folder = temp.path().join(".config/interface/themes");
        let (pushes, mut received) = mpsc::channel(8);
        let poll = ThemeFolderPoll {
            themes: Duration::from_millis(20),
            empty: Duration::from_millis(20),
        };
        let watcher = spawn_theme_folder_watcher(folder.clone(), pushes, poll);
        for _ in 0..200 {
            if folder.is_dir() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(folder.is_dir());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(received.try_recv().is_err(), "no change, no push");
        fs::write(folder.join("Moss.css"), ":root{--accent:#9fd4a8}").unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.operation, "iface:appearanceChanged");
        assert_eq!(frame.payload, serde_json::json!({}));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(received.try_recv().is_err(), "one push per change");
        fs::write(folder.join("notes.txt"), "ignored").unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(received.try_recv().is_err(), "other files are ignored");
        fs::remove_file(folder.join("Moss.css")).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), received.recv())
                .await
                .unwrap()
                .is_some()
        );
        watcher.abort();
    }

    /// Paused time: the poll is asserted on the virtual clock.
    #[tokio::test(start_paused = true)]
    async fn an_empty_folder_is_checked_less_often_than_one_with_themes() {
        let temp = TempDir::new().unwrap();
        let folder = temp.path().join("themes");
        let (pushes, mut received) = mpsc::channel(8);
        let poll = ThemeFolderPoll {
            themes: Duration::from_secs(1),
            empty: Duration::from_secs(5),
        };
        let watcher = spawn_theme_folder_watcher(folder.clone(), pushes, poll);
        while !folder.is_dir() {
            tokio::task::yield_now().await;
        }
        // Let the watcher take its first fingerprint of the empty folder.
        tokio::time::sleep(Duration::from_millis(1)).await;

        // The first theme in an empty folder waits for the slow check.
        fs::write(folder.join("Moss.css"), ":root{--accent:#9fd4a8}").unwrap();
        tokio::time::sleep(Duration::from_millis(4_000)).await;
        assert!(
            received.try_recv().is_err(),
            "an empty folder is checked every 5 s"
        );
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(
            received.try_recv().is_ok(),
            "the first theme file was noticed"
        );

        // With a theme in the folder, an edit is noticed within a second.
        fs::write(folder.join("Moss.css"), ":root{--accent:#a8d49f;}").unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(received.try_recv().is_ok(), "a saved theme was noticed");

        // Once the folder is empty again, it goes back to the slow check.
        fs::remove_file(folder.join("Moss.css")).unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(received.try_recv().is_ok(), "the removal was noticed");
        fs::write(folder.join("Fern.css"), ":root{--accent:#9fd4a8}").unwrap();
        tokio::time::sleep(Duration::from_millis(4_000)).await;
        assert!(
            received.try_recv().is_err(),
            "an empty folder is checked every 5 s"
        );
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(
            received.try_recv().is_ok(),
            "the new theme file was noticed"
        );
        watcher.abort();
    }
}
