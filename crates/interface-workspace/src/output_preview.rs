//! Safe output snapshots. No document executes and no file is written.
use std::path::Path;

use interface_contracts::output_preview::{
    OutputPreviewAction, OutputPreviewContent, OutputPreviewEntry, OutputPreviewRequest,
    OutputPreviewResponse, OutputPreviewSnapshot, OutputTextFormat,
};
use sha2::{Digest, Sha256};

use crate::{
    EDITOR_FILE_BYTE_LIMIT, EditorHooks, WorkspaceAuthority, WorkspaceEditorError,
    is_credential_path, is_likely_binary_path,
};

pub const OUTPUT_TEXT_BYTE_LIMIT: usize = 512 * 1024;
pub const OUTPUT_TEXT_LINE_LIMIT: usize = 10_000;

/// Reads only within currently approved roots. A renderer path never grants access.
/// # Errors
/// Refuses malformed requests, protected paths, links, changing files and missing grants.
pub fn read_output_preview(
    request: &OutputPreviewRequest,
    roots: &[std::path::PathBuf],
) -> Result<OutputPreviewResponse, WorkspaceEditorError> {
    read_output_preview_with_files(request, roots, &[])
}

/// Reads within currently approved roots or exact attached file grants.
/// An exact file grant authorizes reading only that file, never listing its directory
/// or reading sibling files.
/// # Errors
/// Refuses malformed requests, protected paths, links, changing files and missing grants.
pub fn read_output_preview_with_files(
    request: &OutputPreviewRequest,
    roots: &[std::path::PathBuf],
    files: &[std::path::PathBuf],
) -> Result<OutputPreviewResponse, WorkspaceEditorError> {
    request
        .validate()
        .map_err(|_| WorkspaceEditorError::OutsideAuthorizedRoot)?;
    let target = Path::new(&request.root).join(&request.path);
    let authority = WorkspaceAuthority::new();
    // A grant is stored only for an already canonical root, and access needs
    // the canonical target inside it, so a root that is not a lexical prefix
    // of the target could never authorize this read; skip its realpath.
    for root in roots.iter().filter(|root| target.starts_with(root)) {
        // Invalid or replaced roots never become grants.
        let _ = authority.grant_root(root);
    }
    if request.action == OutputPreviewAction::Read {
        for file in files.iter().filter(|file| *file == &target) {
            let _ = authority.grant_file(file);
        }
    }
    preview_with_hooks(&authority, request, None)
}

/// `request` has been validated by the caller.
fn preview_with_hooks(
    authority: &WorkspaceAuthority,
    request: &OutputPreviewRequest,
    hooks: Option<&dyn EditorHooks>,
) -> Result<OutputPreviewResponse, WorkspaceEditorError> {
    // Check the absolute path too: selecting a sensitive directory as a project
    // must not make its formerly sensitive ancestors disappear from policy.
    let root = Path::new(&request.root);
    let target = root.join(&request.path);
    if is_credential_path(&target.to_string_lossy()) {
        return Err(WorkspaceEditorError::ProtectedPath);
    }
    if request.action == OutputPreviewAction::List {
        let entries = authority
            .list_preview_directory(&target)?
            .into_iter()
            .map(|entry| OutputPreviewEntry {
                name: entry.name,
                directory: entry.dir,
            })
            .collect();
        return Ok(OutputPreviewResponse::Directory {
            version: 1,
            root: request.root.clone(),
            path: request.path.clone(),
            entries,
        });
    }
    let size = authority.preview_file_size(&target)?;
    let extension = target
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let unsupported = if size > EDITOR_FILE_BYTE_LIMIT as u64 {
        Some("This file is larger than the 5 MB preview limit. Open it in its own application.")
    } else if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf"
    ) && is_likely_binary_path(&request.path)
    {
        Some("This format has no embedded preview. Open it in its own application.")
    } else {
        None
    };
    let (content, revision, byte_length) = if let Some(reason) = unsupported {
        (
            OutputPreviewContent::Unsupported {
                reason: reason.into(),
            },
            None,
            size,
        )
    } else {
        let bytes = authority.preview_file_bytes(&target, hooks)?;
        let byte_length = bytes.len() as u64;
        let revision = format!("{:x}", Sha256::digest(&bytes));
        let content = content(&extension, bytes);
        let revision =
            (!matches!(content, OutputPreviewContent::Unsupported { .. })).then_some(revision);
        (content, revision, byte_length)
    };
    Ok(OutputPreviewResponse::File {
        version: 1,
        snapshot: OutputPreviewSnapshot {
            root: request.root.clone(),
            path: request.path.clone(),
            byte_length,
            revision,
            content,
        },
    })
}

fn content(extension: &str, bytes: Vec<u8>) -> OutputPreviewContent {
    let media = match extension {
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("image/png"),
        "jpg" | "jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Some("image/jpeg"),
        "gif" if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => Some("image/gif"),
        "webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => {
            Some("image/webp")
        }
        "pdf" if bytes.starts_with(b"%PDF-") => return OutputPreviewContent::Pdf { bytes },
        _ => None,
    };
    if let Some(media_type) = media {
        return OutputPreviewContent::Image {
            media_type: media_type.into(),
            bytes,
        };
    }
    let reason = if matches!(extension, "png" | "jpg" | "jpeg" | "gif" | "webp" | "pdf") {
        "The file contents do not match its image or PDF format."
    } else if bytes.len() > OUTPUT_TEXT_BYTE_LIMIT {
        "This text is larger than the 512 KB preview limit. Open it in its own application."
    } else if bytes
        .iter()
        .filter(|&&byte| byte == b'\n')
        .take(OUTPUT_TEXT_LINE_LIMIT)
        .count()
        >= OUTPUT_TEXT_LINE_LIMIT
    {
        "This text exceeds the 10,000-line preview limit. Open it in its own application."
    } else if bytes.contains(&0) {
        "This file contains binary data and cannot be displayed as text."
    } else if let Ok(text) = String::from_utf8(bytes) {
        let format = match extension {
            "md" | "markdown" => OutputTextFormat::Markdown,
            "diff" | "patch" => OutputTextFormat::Diff,
            _ => OutputTextFormat::Plain,
        };
        return OutputPreviewContent::Text { text, format };
    } else {
        "This file is not valid UTF-8 text."
    };
    OutputPreviewContent::Unsupported {
        reason: reason.into(),
    }
}

#[cfg(test)]
#[path = "output_preview_tests.rs"]
mod tests;
