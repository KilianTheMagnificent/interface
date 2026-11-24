use std::path::{Path, PathBuf};

use interface_common::paths::lexically_normalized;

use interface_contracts::ipc::CHANNELS;

use crate::{DesktopServiceError, payload::MovedFolderPayload};

/// The most folder moves one roster save may carry.
pub(crate) const MAX_MOVED_FOLDERS: usize = 16;

/// The real directory a saved folder now leads to, when the saved path is no
/// longer that directory itself: it became a link, or sits under one. The
/// terminal gate refuses such a path, so a pane saved there cannot reopen until
/// the owner confirms the new place. `None` for a path that is already the
/// real directory, or one that leads nowhere.
pub(crate) fn moved_folder_target(saved: &Path) -> Option<PathBuf> {
    if !saved.is_absolute() || lexically_normalized(saved) != saved {
        return None;
    }
    let target = std::fs::canonicalize(saved).ok()?;
    if target == saved {
        return None;
    }
    let metadata = std::fs::symlink_metadata(&target).ok()?;
    (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some(target)
}

/// Re-proves one confirmed move against the disk as it is now. The owner saw
/// `to`; if `from` leads anywhere else by the time the save arrives, nothing
/// moves.
pub(crate) fn verified_move(
    from: &str,
    to: &str,
) -> Result<(PathBuf, PathBuf), DesktopServiceError> {
    let from = PathBuf::from(from);
    match moved_folder_target(&from) {
        Some(target) if target == Path::new(to) => Ok((from, target)),
        _ => Err(DesktopServiceError::new(
            "terminal-folder-move-changed",
            format!(
                "{} no longer leads to {to}. Nothing was moved.",
                from.display()
            ),
            false,
        )),
    }
}

/// Re-proves every move a roster save carries, refusing the whole save when
/// any one of them no longer holds.
pub(crate) fn verified_moves(
    moves: &[MovedFolderPayload],
) -> Result<Vec<(PathBuf, PathBuf)>, DesktopServiceError> {
    if moves.len() > MAX_MOVED_FOLDERS {
        return Err(DesktopServiceError::invalid_payload(
            CHANNELS.set_terminal_workspaces,
        ));
    }
    moves
        .iter()
        .map(|moved| verified_move(&moved.from, &moved.to))
        .collect()
}
