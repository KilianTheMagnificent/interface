//! Cancellable argv-only checks. Dropping a deadline/Stop future kills the group.
use serde_json::{Value, json};
use std::{path::Path, process::Stdio};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

struct Group(u32);
impl Drop for Group {
    fn drop(&mut self) {
        // The check was spawned as its group's leader, so its pid names the
        // group; a group that already exited is fine.
        if let Some(group) = i32::try_from(self.0)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
    }
}
async fn capture(pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    pipe.take(16_385)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16_384 {
        return Err("Check output exceeded its bound; no result accepted".into());
    }
    Ok(bytes)
}
pub(super) async fn run(argv: &[String], cwd: &Path) -> Result<Value, String> {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let _group = Group(child.id().ok_or("Check process identity unavailable")?);
    let stdout = child.stdout.take().ok_or("Check stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Check stderr unavailable")?;
    let (out, err, status) = tokio::try_join!(capture(stdout), capture(stderr), async {
        child.wait().await.map_err(|e| e.to_string())
    })?;
    Ok(
        json!({"argv":argv,"exitCode":status.code(),"passed":status.success(),"stdout":String::from_utf8_lossy(&out),"stderr":String::from_utf8_lossy(&err)}),
    )
}
