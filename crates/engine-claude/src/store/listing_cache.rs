//! Incremental display summaries with complete-prefix verification. This avoids
//! reparsing old records without trusting file growth as proof of append-only I/O.
use super::cache_file::{
    PrefixHash, next_record, open_checked, same_identity, same_version, verify_prefix,
    verify_unchanged,
};
use super::{
    INTERFACE_SESSION_TAG, MAX_TITLE_CHARS, clip, epoch_iso, listing_line_may_matter, nonempty,
    system_time_iso, user_turn_text,
};
use interface_contracts::engine::{EngineId, SessionOrigin, SessionStatus, SessionSummary};
use serde_json::Value;
use std::{
    fs::Metadata,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Debug, Default)]
struct Fields {
    generated_summary: Option<String>,
    custom_title: Option<String>,
    first_prompt: Option<String>,
    cwd: Option<String>,
    tag: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct CachedSummary {
    metadata: Metadata,
    offset: u64,
    fingerprint: u64,
    fields: Fields,
    pub(super) summary: SessionSummary,
    #[cfg(test)]
    parsed_bytes: u64,
}

impl CachedSummary {
    /// Whether `named` still shows the transcript version this entry read.
    pub(super) fn is_current(&self, named: &Metadata) -> bool {
        same_version(named, &self.metadata)
    }

    #[cfg(test)]
    pub(super) fn parsed_bytes(&self) -> u64 {
        self.parsed_bytes
    }
}

pub(super) fn read(
    path: &Path,
    named: &Metadata,
    engine: &EngineId,
    session_id: &str,
    previous: Option<CachedSummary>,
) -> io::Result<CachedSummary> {
    // The previous listing's entry is moved in, so an unchanged transcript
    // is carried forward without copying its summary.
    let previous = match previous {
        Some(old) if same_version(named, &old.metadata) => {
            #[cfg(test)]
            let old = CachedSummary {
                parsed_bytes: 0,
                ..old
            };
            return Ok(old);
        }
        other => other,
    };
    let (mut file, metadata) = open_checked(path, named)?;
    let mut fields = Fields::default();
    let mut offset = 0;
    let mut hash = PrefixHash::new();
    if let Some(old) = previous
        .filter(|old| same_identity(&old.metadata, &metadata) && old.offset <= metadata.len())
        && let Some(checked) = verify_prefix(&mut file, old.offset, old.fingerprint)?
    {
        offset = old.offset;
        fields = old.fields;
        hash = checked;
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new((&mut file).take(metadata.len() - offset));
    let mut line = Vec::new();
    let mut partial = None;
    #[cfg(test)]
    let start = offset;
    loop {
        let count = next_record(&mut reader, &mut line)?;
        if count == 0 {
            break;
        }
        let text = std::str::from_utf8(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if line.last() != Some(&b'\n') {
            // A valid last record remains visible, but its bytes are retried
            // when more data arrives, just like a partial JSON record.
            let mut display = fields.clone();
            display.observe(text);
            partial = Some(display);
            break;
        }
        fields.observe(text);
        offset += count as u64;
        hash.write(&line);
    }
    verify_unchanged(
        path,
        &file,
        &metadata,
        "transcript changed during summary read",
    )?;
    let summary = partial
        .as_ref()
        .unwrap_or(&fields)
        .summary(&metadata, engine, session_id);
    Ok(CachedSummary {
        metadata,
        offset,
        fingerprint: hash.finish(),
        fields,
        summary,
        #[cfg(test)]
        parsed_bytes: offset - start + line.len() as u64,
    })
}

impl Fields {
    fn observe(&mut self, line: &str) {
        let line = line.trim();
        if !listing_line_may_matter(line, self.cwd.is_none(), self.first_prompt.is_none()) {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(record) = value.as_object() else {
            return;
        };
        if self.cwd.is_none() {
            self.cwd = nonempty(record.get("cwd")).map(ToOwned::to_owned);
        }
        if let Some(summary) = nonempty(record.get("summary")) {
            self.generated_summary = Some(clip(summary, MAX_TITLE_CHARS));
        }
        match nonempty(record.get("type")) {
            Some("custom-title") => {
                self.custom_title =
                    nonempty(record.get("customTitle")).map(|s| clip(s, MAX_TITLE_CHARS));
            }
            Some("tag") => self.tag = nonempty(record.get("tag")).map(ToOwned::to_owned),
            Some("user") if self.first_prompt.is_none() => {
                self.first_prompt = user_turn_text(record.get("message")).map(|v| clip(&v, 80));
            }
            _ => {}
        }
    }

    fn summary(&self, metadata: &Metadata, engine: &EngineId, session_id: &str) -> SessionSummary {
        let title = self
            .generated_summary
            .as_deref()
            .or(self.custom_title.as_deref())
            .or(self.first_prompt.as_deref())
            .map_or_else(|| session_id.to_owned(), |v| clip(v, MAX_TITLE_CHARS));
        let last_activity = metadata
            .modified()
            .ok()
            .map_or_else(epoch_iso, system_time_iso);
        let first_seen = metadata
            .created()
            .ok()
            .map_or_else(|| last_activity.clone(), system_time_iso);
        SessionSummary {
            engine: engine.clone(),
            session_id: session_id.to_owned(),
            title,
            cwd: self.cwd.clone().unwrap_or_default(),
            status: SessionStatus::Idle,
            cloud: false,
            model: None,
            effort: None,
            last_activity,
            first_seen,
            archived: false,
            size: Some(metadata.len()),
            title_from_summary: Some(
                self.generated_summary.is_some() || self.custom_title.is_some(),
            ),
            origin: Some(if self.tag.as_deref() == Some(INTERFACE_SESSION_TAG) {
                SessionOrigin::App
            } else {
                SessionOrigin::Store
            }),
            project_root: None,
            worktree_home: None,
            worktree_path: None,
            setting_sources: None,
            disallowed_tools: None,
            kind: None,
            permission_mode: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::safe_file_metadata;
    use super::*;
    use std::{fs::OpenOptions, io::Write};

    fn load(path: &Path, old: Option<&CachedSummary>) -> CachedSummary {
        read(
            path,
            &safe_file_metadata(path).unwrap(),
            &EngineId::new("claude").unwrap(),
            "fixture",
            old.cloned(),
        )
        .unwrap()
    }

    #[test]
    fn append_parses_only_new_records_and_retries_valid_and_invalid_partial_eof() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.jsonl");
        let row = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"content\":\"{}\"}}}}\n",
            "x".repeat(1024)
        );
        std::fs::write(&path, format!("{{\"type\":\"user\",\"cwd\":\"/fixture\",\"message\":{{\"content\":\"First\"}}}}\n{}", row.repeat(2048))).unwrap();
        let first = load(&path, None);
        assert_eq!(first.summary.title, "First");
        assert_eq!(load(&path, Some(&first)).parsed_bytes, 0);
        let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
        writer
            .write_all(b"{\"type\":\"custom-title\",\"customTitle\":\"Next\"}")
            .unwrap();
        let partial = load(&path, Some(&first));
        assert_eq!(partial.summary.title, "Next");
        assert!(partial.parsed_bytes < 100);
        assert_eq!(partial.offset, first.offset);
        writer
            .write_all(b"\n{\"type\":\"summary\",\"summary\":\"")
            .unwrap();
        let invalid = load(&path, Some(&partial));
        assert_eq!(invalid.summary.title, "Next");
        assert!(invalid.parsed_bytes < 100);
        writer.write_all(b"Summarized\"}\n").unwrap();
        let complete = load(&path, Some(&invalid));
        assert_eq!(complete.summary.title, "Summarized");
        assert_eq!(complete.summary, load(&path, None).summary);
        assert!(complete.parsed_bytes < 100);
    }

    #[test]
    fn rewrite_with_growth_truncation_and_inode_replacement_match_a_fresh_reader() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"custom-title\",\"customTitle\":\"First\"}\n",
        )
        .unwrap();
        let mut old = load(&path, None);
        for content in [
            "{\"type\":\"custom-title\",\"customTitle\":\"Rewritten longer\"}\n",
            "{\"type\":\"custom-title\",\"customTitle\":\"Short\"}\n",
        ] {
            std::fs::write(&path, content).unwrap();
            old = load(&path, Some(&old));
            assert_eq!(old.summary, load(&path, None).summary);
        }
        let replacement = temp.path().join("replacement");
        std::fs::write(
            &replacement,
            b"{\"type\":\"custom-title\",\"customTitle\":\"Other\"}\n",
        )
        .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(load(&path, Some(&old)).summary, load(&path, None).summary);
    }
}
