//! Structured proposals from workers that receive content instead of file tools.
use super::{
    Change, Contents, MAX_FILE_BYTES, MAX_RESULT_BYTES, Packet, add_bytes, assert_directory,
    digest, read_file,
};
use std::collections::BTreeSet;

/// One worker-proposed file; `after: None` deletes it. Built from engine
/// results in memory, never (de)serialized.
#[derive(Clone, Debug)]
pub struct ProposedFile {
    pub path: String,
    pub after: Option<String>,
}

impl Packet {
    /// Captured, already filtered content for a worker without filesystem tools.
    #[must_use]
    pub fn content_manifest(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.entries
                .iter()
                .map(|(path, entry)| {
                    serde_json::json!({
                        "path": path, "writable": entry.writable,
                        "content": entry.before.as_ref().map(|c| &c.text)
                    })
                })
                .collect(),
        )
    }

    /// Convert a content-only response into the ordinary frozen proposal.
    /// Nothing is written, including to the private packet. Worker-provided
    /// modes, hashes, source identities and additional paths are never trusted.
    ///
    /// # Errors
    /// Refuses duplicate/out-of-scope paths, exceeded bounds, or changed roots.
    pub fn propose(&self, files: &[ProposedFile]) -> Result<Vec<Change>, String> {
        if files.len() > 16 {
            return Err("worker proposed too many files".into());
        }
        if !self.collect()?.is_empty() {
            return Err("content-only worker packet was modified".into());
        }
        let mut seen = BTreeSet::new();
        let mut bytes = 0;
        let mut changes = Vec::new();
        for file in files {
            let entry = self
                .entries
                .get(&file.path)
                .filter(|e| e.writable)
                .ok_or("worker proposed a file outside its writable scope")?;
            if !seen.insert(&file.path) {
                return Err("worker proposed a duplicate file".into());
            }
            let after = file.after.as_ref().map(|text| Contents {
                text: text.clone(),
                mode: entry.before.as_ref().map_or(0o600, |c| c.mode),
            });
            if file
                .after
                .as_ref()
                .is_some_and(|s| s.len() > MAX_FILE_BYTES || s.contains('\0'))
            {
                return Err("worker proposed oversized or binary content".into());
            }
            add_bytes(&mut bytes, after.as_ref(), MAX_RESULT_BYTES)?;
            if after == entry.before {
                continue;
            }
            let current = read_file(&self.source, &file.path)?;
            changes.push(Change {
                path: file.path.clone(),
                before: entry.before.as_ref().map(|c| c.text.clone()),
                before_sha256: entry.before.as_ref().map(|c| digest(&c.text)),
                after: file.after.clone(),
                source_changed: current != entry.before,
                before_mode: entry.before.as_ref().map(|c| c.mode),
                after_mode: after.map(|c| c.mode),
            });
        }
        assert_directory(&self.source)?;
        assert_directory(&self.workspace)?;
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn content_proposals_are_scoped_stale_aware_and_never_written() {
        let root = tempfile::tempdir_in("/private/tmp").unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("a.rs"), "before").unwrap();
        fs::write(source.join("context.rs"), "context").unwrap();
        let packet = super::super::prepare(
            &source,
            &root.path().join("packet"),
            &["a.rs".into(), "new.rs".into()],
            &["context.rs".into()],
        )
        .unwrap();
        assert_eq!(packet.content_manifest()[0]["content"], "before");
        fs::write(source.join("a.rs"), "parent edit").unwrap();
        let change = ProposedFile {
            path: "a.rs".into(),
            after: Some("after\n\"quoted\"".into()),
        };
        let proposed = packet.propose(std::slice::from_ref(&change)).unwrap();
        assert!(proposed[0].source_changed);
        assert_eq!(proposed[0].before.as_deref(), Some("before"));
        assert_eq!(
            fs::read_to_string(source.join("a.rs")).unwrap(),
            "parent edit"
        );
        assert_eq!(
            fs::read_to_string(packet.workspace().join("a.rs")).unwrap(),
            "before"
        );
        assert!(packet.propose(&[change.clone(), change]).is_err());
        for path in ["context.rs", "../a.rs", ".env", "unlisted.rs", "/tmp/file"] {
            assert!(
                packet
                    .propose(&[ProposedFile {
                        path: path.into(),
                        after: Some("bad".into())
                    }])
                    .is_err()
            );
        }
        assert!(
            packet
                .propose(&[ProposedFile {
                    path: "new.rs".into(),
                    after: Some("\0".into())
                }])
                .is_err()
        );
        assert!(
            packet
                .propose(&[ProposedFile {
                    path: "new.rs".into(),
                    after: Some("x".repeat(MAX_RESULT_BYTES + 1))
                }])
                .is_err()
        );
        let deletion = packet
            .propose(&[ProposedFile {
                path: "a.rs".into(),
                after: None,
            }])
            .unwrap();
        assert_eq!(deletion[0].after_mode, None);
        let creation = packet
            .propose(&[ProposedFile {
                path: "new.rs".into(),
                after: Some("new".into()),
            }])
            .unwrap();
        assert_eq!(creation[0].before, None);
        assert_eq!(creation[0].after_mode, Some(0o600));
        fs::write(packet.workspace().join("context.rs"), "modified").unwrap();
        assert!(packet.propose(&[]).is_err());
    }
}
