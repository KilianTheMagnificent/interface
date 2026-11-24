//! Move acknowledged evidence out of the hot index without forgetting retries.
use super::{MAX_RECORDS, Record, Service, State, digest, limits, valid_id};
use interface_storage::{TargetSnapshot, atomic_write_json, read_json};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

impl Service {
    fn history_read<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>, String> {
        read_json::<T>(&self.directory, name, limits())
            .map(|value| value.map(|v| v.into_parts().0))
            .map_err(|e| format!("storageUncertain: archive unavailable; {e}"))
    }
    fn history_write<T: Serialize + serde::de::DeserializeOwned>(
        &self,
        state: &mut State,
        name: &str,
        value: &T,
    ) -> Result<(), String> {
        let result = (|| {
            if let Some(old) = self.history_read::<T>(name)? {
                if digest(&old) == digest(value) {
                    return Ok(());
                }
                return Err("storageUncertain: archive content conflict".into());
            }
            atomic_write_json(
                &self.directory,
                &self.writer,
                name,
                value,
                &TargetSnapshot::Missing,
                limits(),
            )
            .map(|_| ())
            .map_err(|e| format!("storageUncertain: archive write failed; {e}"))
        })();
        if let Err(error) = &result {
            state.index = state.committed.clone();
            state.closed = true;
            state.storage_error = Some(error.clone());
        }
        result
    }
    pub(super) fn archived_task(&self, owner: &str, id: &str) -> Result<Record, String> {
        if Uuid::parse_str(id).is_err() {
            return Err("scopeDenied: invalid archived task".into());
        }
        let record: Record = self
            .history_read(&format!("archive-{id}.json"))?
            .ok_or("scopeDenied: task is not owned by this conversation")?;
        if record.owner != owner
            || record.task_id != id
            || !record.received
            || !record.state.terminal()
            || record.result.as_ref().map(digest) != record.result_sha256
        {
            return Err("scopeDenied: archived task is unavailable".into());
        }
        Ok(record)
    }
    pub(super) fn archived_request(&self, owner: &str, id: &str) -> Result<Option<Record>, String> {
        let task: Option<String> = self.history_read(&receipt_name("start", owner, id))?;
        task.map(|task| self.archived_task(owner, &task))
            .transpose()
    }
    pub(super) fn refuse_mutation_id(
        &self,
        state: &State,
        owner: &str,
        id: &str,
    ) -> Result<(), String> {
        if state.index.mutations.contains_key(&format!("{owner}/{id}"))
            || self
                .history_read::<(String, Value)>(&receipt_name("mutation", owner, id))?
                .is_some()
        {
            return Err("idempotencyConflict: requestId already used by another mutation".into());
        }
        Ok(())
    }
    pub(super) fn replay(
        &self,
        state: &State,
        owner: &str,
        id: &str,
        intent: &Value,
    ) -> Result<Option<Value>, String> {
        if !valid_id(id) {
            return Err("invalidRequest: invalid requestId".into());
        }
        if state
            .index
            .records
            .values()
            .any(|r| r.owner == owner && r.request.request_id == id)
            || self.archived_request(owner, id)?.is_some()
        {
            return Err("idempotencyConflict: requestId already used to start a task".into());
        }
        let prior = state
            .index
            .mutations
            .get(&format!("{owner}/{id}"))
            .cloned()
            .or(self.history_read(&receipt_name("mutation", owner, id))?);
        prior
            .map(|(hash, value)| {
                if hash == digest(intent) {
                    Ok(value)
                } else {
                    Err("idempotencyConflict: mutation requestId reused".into())
                }
            })
            .transpose()
    }
    pub(super) fn compact_mutations(&self, state: &mut State) -> Result<(), String> {
        if self.archive_mutations(state)? {
            self.persist(state)?;
        }
        Ok(())
    }
    /// Moves the oldest receipts out of a crowded hot index without persisting
    /// it. Returns whether any moved.
    fn archive_mutations(&self, state: &mut State) -> Result<bool, String> {
        if state.index.mutations.len() < 1024 {
            return Ok(false);
        }
        let entries: Vec<_> = state
            .index
            .mutations
            .iter()
            .take(512)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, receipt) in entries {
            let (owner, id) = key
                .split_once('/')
                .ok_or("storageUncertain: invalid receipt owner")?;
            self.history_write(state, &receipt_name("mutation", owner, id), &receipt)?;
            state.index.mutations.remove(&key);
        }
        Ok(true)
    }
    pub(super) fn compact_history(&self, state: &mut State) -> Result<(), String> {
        if state.index.records.len() < MAX_RECORDS {
            return Ok(());
        }
        let mut eligible: Vec<_> = state
            .index
            .records
            .values()
            .filter(|r| r.received && r.state.terminal())
            .cloned()
            .collect();
        eligible.sort_by_key(|r| r.sequence);
        let mut moved = false;
        for record in eligible.into_iter().take(32) {
            // Every archive and retry receipt commits before the hot record is removed.
            self.history_write(state, &format!("archive-{}.json", record.task_id), &record)?;
            self.history_write(
                state,
                &receipt_name("start", &record.owner, &record.request.request_id),
                &record.task_id,
            )?;
            state.index.records.remove(&record.task_id);
            moved = true;
        }
        moved |= self.archive_mutations(state)?;
        // A full index of unreceived results moves nothing; the committed index
        // is already on disk, so the refusal that follows rewrites nothing.
        if moved {
            self.persist(state)?;
        }
        Ok(())
    }
}
fn receipt_name(kind: &str, owner: &str, id: &str) -> String {
    format!("{kind}-{}.json", digest(&(owner, id)))
}
