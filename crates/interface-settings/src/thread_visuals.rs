//! Visual replies' pages (T3 section 8, protocol 61): the self-contained
//! HTML documents agents show through team tools' `show_visual`, kept per
//! thread beside its journal. Contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 8.
//!
//! `<data root>/thread-visuals/<h>/<id>.html`, `<h>` the journal's thread
//! directory name for the session key ([`thread_hash`]), `<id>` a
//! [`is_visual_id`] id (oldest sorts first). Owner-private directories and
//! files. One [`ThreadVisuals`] writes (the journal worker, in the process
//! holding `host.lock`, which also holds this directory's writer lock);
//! [`read_visual`] takes no lock.
//!
//! The rules are pure and tested on their own: [`check_visual`] (title,
//! size, "is it HTML"), [`visual_id`] and [`retention`] (at most
//! [`MAX_THREAD_VISUALS`] pages and [`MAX_THREAD_VISUAL_BYTES`] bytes a
//! thread, oldest removed first).

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use interface_contracts::thread_journal::{MAX_VISUAL_TITLE_CHARS, is_visual_id};
use interface_storage::{PrivateDirectory, WriterLock};

use crate::{SettingsError, thread_journal::thread_hash};

/// The store's directory under the data root.
pub const VISUALS_DIR: &str = "thread-visuals";
/// The largest page, in UTF-8 bytes.
pub const MAX_VISUAL_BYTES: usize = 512 * 1024;
/// A thread keeps at most this many pages...
pub const MAX_THREAD_VISUALS: usize = 64;
/// ...and at most this many bytes of them.
pub const MAX_THREAD_VISUAL_BYTES: u64 = 16 * 1024 * 1024;

const LOCK_NAME: &str = "visuals.lock";
const PAGE_SUFFIX: &str = ".html";
const TEMP_SUFFIX: &str = ".tmp";

/// Why a page was not kept. The model reads [`VisualRefusal::code`] and
/// [`VisualRefusal::message`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VisualRefusal {
    /// The page is over [`MAX_VISUAL_BYTES`].
    TooLarge { bytes: usize },
    /// The text is not an HTML document (or carries a NUL).
    NotHtml,
    /// The title is empty, too long or holds control characters.
    InvalidTitle,
}

impl VisualRefusal {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "visual-too-large",
            Self::NotHtml => "visual-not-html",
            Self::InvalidTitle => "invalid-input",
        }
    }

    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::TooLarge { bytes } => format!(
                "The page is {bytes} bytes; a visual may be at most {MAX_VISUAL_BYTES} bytes \
                 (512 KiB). Make it smaller: fewer data points, no embedded fonts, smaller \
                 images."
            ),
            Self::NotHtml => "`html` must be one HTML document: start it with `<!doctype html>` \
                              or a tag such as `<svg>` or `<div>`, with no NUL characters."
                .to_owned(),
            Self::InvalidTitle => {
                format!("`title` must be 1 to {MAX_VISUAL_TITLE_CHARS} characters on one line.")
            }
        }
    }
}

/// Whether `title` and `html` may be shown. `html` is a Rust string, so it
/// is UTF-8 by construction; its size is its UTF-8 length.
///
/// # Errors
/// The first rule the page breaks, in the order title, size, HTML.
pub fn check_visual(title: &str, html: &str) -> Result<(), VisualRefusal> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > MAX_VISUAL_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        return Err(VisualRefusal::InvalidTitle);
    }
    if html.len() > MAX_VISUAL_BYTES {
        return Err(VisualRefusal::TooLarge { bytes: html.len() });
    }
    if !looks_like_html(html) {
        return Err(VisualRefusal::NotHtml);
    }
    Ok(())
}

/// Whether `text` reads as an HTML (or SVG) document: after whitespace and
/// a byte-order mark it starts with `<`, it holds an element's start tag
/// (`<` and a letter) and no NUL.
#[must_use]
pub fn looks_like_html(text: &str) -> bool {
    let body = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    body.starts_with('<')
        && !text.contains('\0')
        && body
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'<' && pair[1].is_ascii_alphabetic())
}

/// A new visual's id: `v`, `at` in 13 digits, `-`, the first 8 hex of
/// sha256(`salt`). Ids sort by the time they were shown.
#[must_use]
pub fn visual_id(at: u64, salt: &[u8]) -> String {
    let hash = interface_storage::sha256_hex(salt);
    format!("v{:013}-{}", at.min(9_999_999_999_999), &hash[..8])
}

/// A thread's bounds; tests shrink them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisualLimits {
    pub count: usize,
    pub bytes: u64,
}

impl Default for VisualLimits {
    fn default() -> Self {
        Self {
            count: MAX_THREAD_VISUALS,
            bytes: MAX_THREAD_VISUAL_BYTES,
        }
    }
}

/// The pages to remove, oldest first, so a new page of `incoming` bytes
/// fits a thread already holding `existing` (`(id, bytes)`, any order).
#[must_use]
pub fn retention(existing: &[(String, u64)], incoming: u64, limits: VisualLimits) -> Vec<String> {
    let mut kept = existing.to_vec();
    kept.sort();
    let mut count = kept.len();
    let mut bytes = kept.iter().map(|(_, size)| *size).sum::<u64>();
    let mut removed = Vec::new();
    for (id, size) in kept {
        if count < limits.count && bytes.saturating_add(incoming) <= limits.bytes {
            break;
        }
        count -= 1;
        bytes = bytes.saturating_sub(size);
        removed.push(id);
    }
    removed
}

/// A page the store kept, and the older ones it removed to make room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedVisual {
    pub bytes: u64,
    pub removed: Vec<String>,
}

/// The one writer.
pub struct ThreadVisuals {
    root: PathBuf,
    directory: PrivateDirectory,
    _lock: WriterLock,
    limits: VisualLimits,
}

impl ThreadVisuals {
    /// Opens (creating) `<data_root>/thread-visuals` and takes its writer
    /// lock.
    ///
    /// # Errors
    /// A relative path, an unsafe directory, or a lock another writer holds.
    pub fn open(data_root: &Path) -> Result<Self, SettingsError> {
        Self::open_with(data_root, VisualLimits::default())
    }

    /// [`ThreadVisuals::open`] with other bounds.
    ///
    /// # Errors
    /// As [`ThreadVisuals::open`].
    pub fn open_with(data_root: &Path, limits: VisualLimits) -> Result<Self, SettingsError> {
        if !data_root.is_absolute() {
            return Err(SettingsError::InvalidPath);
        }
        let root = data_root.join(VISUALS_DIR);
        let directory = PrivateDirectory::open_or_create(&root)?;
        let lock = WriterLock::acquire(&directory, LOCK_NAME)?;
        Ok(Self {
            root,
            directory,
            _lock: lock,
            limits,
        })
    }

    /// Keeps `html` as `key`'s visual `id`, after removing the oldest pages
    /// past the thread's bounds. Checks the page again ([`check_visual`]'s
    /// size and HTML rules) whatever the caller did.
    ///
    /// # Errors
    /// An invalid id or page, an id already kept, or a write that failed
    /// (nothing is kept then; removals already made stay made).
    pub fn save(&mut self, key: &str, id: &str, html: &str) -> Result<SavedVisual, SettingsError> {
        if !is_visual_id(id) {
            return Err(SettingsError::UnsafeStorage("invalid visual id"));
        }
        if html.len() > MAX_VISUAL_BYTES || !looks_like_html(html) {
            return Err(SettingsError::UnsafeStorage("invalid visual page"));
        }
        let hash = thread_hash(key);
        let thread = self.directory.open_or_create_child(&hash)?;
        let dir = self.root.join(&hash);
        let existing = list_pages(&dir)?;
        if existing.iter().any(|(kept, _)| kept == id) {
            return Err(SettingsError::UnsafeStorage("visual id already kept"));
        }
        let incoming = html.len() as u64;
        let removed = retention(&existing, incoming, self.limits);
        for old in &removed {
            match fs::remove_file(dir.join(format!("{old}{PAGE_SUFFIX}"))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into());
                }
                _ => {}
            }
        }
        let temp = dir.join(format!(".{id}{TEMP_SUFFIX}"));
        let written = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)?;
            file.write_all(html.as_bytes())?;
            file.sync_data()?;
            fs::rename(&temp, dir.join(format!("{id}{PAGE_SUFFIX}")))
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        thread.sync()?;
        Ok(SavedVisual {
            bytes: incoming,
            removed,
        })
    }

    /// Removes every page of `key` (its session was deleted or forgotten).
    ///
    /// # Errors
    /// The directory could not be removed.
    pub fn remove_thread(&mut self, key: &str) -> Result<(), SettingsError> {
        match fs::remove_dir_all(self.root.join(thread_hash(key))) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// `key`'s kept pages, `(id, bytes)` oldest first.
    ///
    /// # Errors
    /// The thread's directory could not be read.
    pub fn pages(&self, key: &str) -> Result<Vec<(String, u64)>, SettingsError> {
        let mut pages = list_pages(&self.root.join(thread_hash(key)))?;
        pages.sort();
        Ok(pages)
    }
}

/// `(id, bytes)` of the pages in `dir`, unordered; leftover temporary files
/// of an interrupted save are removed. A missing directory holds none.
fn list_pages(dir: &Path) -> Result<Vec<(String, u64)>, SettingsError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut pages = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') && name.ends_with(TEMP_SUFFIX) {
            let _ = fs::remove_file(entry.path());
            continue;
        }
        let Some(id) = name.strip_suffix(PAGE_SUFFIX) else {
            continue;
        };
        let metadata = entry.metadata()?;
        if is_visual_id(id) && metadata.is_file() {
            pages.push((id.to_owned(), metadata.len()));
        }
    }
    Ok(pages)
}

/// `key`'s visual `id`, if the store keeps it: a regular file (never a
/// link) of at most [`MAX_VISUAL_BYTES`] valid UTF-8 bytes. Takes no lock.
#[must_use]
pub fn read_visual(data_root: &Path, key: &str, id: &str) -> Option<String> {
    if !is_visual_id(id) {
        return None;
    }
    let path = data_root
        .join(VISUALS_DIR)
        .join(thread_hash(key))
        .join(format!("{id}{PAGE_SUFFIX}"));
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_VISUAL_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_VISUAL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_VISUAL_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
#[path = "thread_visuals_tests.rs"]
mod tests;
