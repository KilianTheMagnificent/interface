//! Spawns and supervises the `interface-voice` helper over NDJSON stdio.

use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use interface_engine::jsonrpc::NdjsonDecoder;
use interface_voice::{HelperToHost, HostToHelper};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, Command},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub(crate) const HELPER_PATH_ENV: &str = "INTERFACE_VOICE_HELPER";
const HELPER_BINARY: &str = "interface-voice";
const EVENT_CAPACITY: usize = 32;
const MAX_HELPER_FRAME_BYTES: usize = 1024 * 1024;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(crate) enum HelperEvent {
    Message(HelperToHost),
    Exited,
}

pub(crate) struct Helper {
    stdin: ChildStdin,
    kill: Option<oneshot::Sender<()>>,
    supervisor: JoinHandle<()>,
}

/// `INTERFACE_VOICE_HELPER` when set, else `interface-voice` beside the host.
pub(crate) fn helper_path() -> PathBuf {
    if let Some(path) = std::env::var_os(HELPER_PATH_ENV) {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HELPER_BINARY)))
        .unwrap_or_else(|| PathBuf::from(HELPER_BINARY))
}

/// Spawns the helper with piped stdio and starts its reader, stderr and
/// supervisor tasks. Must be called inside a Tokio runtime.
pub(crate) fn spawn(path: &Path) -> io::Result<(Helper, mpsc::Receiver<HelperEvent>)> {
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("voice helper stdin pipe was not created"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("voice helper stdout pipe was not created"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("voice helper stderr pipe was not created"))?;
    let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
    let reader_events = events.clone();
    let mut reader = tokio::spawn(read_messages(stdout, reader_events));
    let stderr_task = tokio::spawn(async move {
        // Diagnostics are untrusted, potentially sensitive, and need not be
        // line-framed. Drain fixed chunks without retaining or mirroring them.
        let mut stderr = stderr;
        let mut buffer = [0; 8192];
        while let Ok(count) = stderr.read(&mut buffer).await {
            if count == 0 {
                break;
            }
        }
    });
    let (kill, kill_signal) = oneshot::channel();
    let supervisor = tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                tracing::info!(?status, "voice helper exited");
            }
            _ = kill_signal => {
                let _ = child.kill().await;
            }
            _ = &mut reader => {
                // A broken/oversized protocol stream cannot leave a live
                // helper waiting forever after its consumer has exited.
                let _ = child.kill().await;
            }
        }
        reader.abort();
        stderr_task.abort();
        let _ = events.send(HelperEvent::Exited).await;
    });
    Ok((
        Helper {
            stdin,
            kill: Some(kill),
            supervisor,
        },
        receiver,
    ))
}

async fn read_messages(
    mut stdout: impl AsyncRead + Unpin,
    events: mpsc::Sender<HelperEvent>,
) -> io::Result<()> {
    let mut decoder = NdjsonDecoder::new(MAX_HELPER_FRAME_BYTES);
    let mut buffer = [0; 8192];
    loop {
        let count = stdout.read(&mut buffer).await?;
        let frames = if count == 0 {
            decoder.finish()
        } else {
            decoder.push(&buffer[..count])
        };
        for frame in frames {
            let value = frame.map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid voice helper frame")
            })?;
            let message = serde_json::from_value(value).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid voice helper message")
            })?;
            if events.send(HelperEvent::Message(message)).await.is_err() {
                return Ok(());
            }
        }
        if count == 0 {
            return Ok(());
        }
    }
}

impl Helper {
    /// Writes one NDJSON line to the helper.
    pub(crate) async fn send(&mut self, message: &HostToHelper) -> io::Result<()> {
        let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
        if line.len() > MAX_HELPER_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "voice helper frame exceeds its byte limit",
            ));
        }
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await
    }

    /// Waits up to two seconds for the helper to exit on its own, then kills it.
    pub(crate) async fn stop(mut self) {
        drop(self.stdin);
        if tokio::time::timeout(SHUTDOWN_GRACE, &mut self.supervisor)
            .await
            .is_ok()
        {
            return;
        }
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
        let _ = self.supervisor.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn helper_framing_bounds_unterminated_lines_and_keeps_final_frames() {
        let (events, mut receiver) = mpsc::channel(2);
        read_messages(
            b"{\"type\":\"interrupted\"}\n{\"type\":\"interrupted\"}".as_slice(),
            events,
        )
        .await
        .unwrap();
        assert!(matches!(
            receiver.recv().await,
            Some(HelperEvent::Message(_))
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(HelperEvent::Message(_))
        ));
        let (events, _receiver) = mpsc::channel(2);
        let oversized = vec![b'x'; MAX_HELPER_FRAME_BYTES + 1];
        assert_eq!(
            read_messages(oversized.as_slice(), events)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
