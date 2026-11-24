//! Reading a child's output until the child is done, rather than until every
//! copy of its pipe is closed. On macOS a pipe becomes close-on-exec only
//! after it is created, so a child another thread spawns in that moment can
//! inherit it and hold it open long after this child exits; see
//! [`interface_common::process::PIPE_IDLE_AFTER_EXIT`].

use std::{io, process::ExitStatus};

use interface_common::process::{PIPE_IDLE_AFTER_EXIT, process_group_is_empty};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Child,
    sync::watch,
    time::timeout,
};

/// Waits for `child` to exit, then tells the readers of its pipes.
///
/// # Errors
///
/// The child could not be waited on.
pub async fn wait_and_tell(
    child: &mut Child,
    exited: watch::Sender<bool>,
) -> io::Result<ExitStatus> {
    let status = child.wait().await;
    let _ = exited.send(true);
    status
}

/// Reads `pipe` into `keep` until its end, or, once `exited` says the child
/// exited and the process `group` it led (if any) is empty, until the pipe
/// stays empty for [`PIPE_IDLE_AFTER_EXIT`].
///
/// # Errors
///
/// A read error, or the first error `keep` returns.
pub async fn read_until_done(
    pipe: &mut (impl AsyncRead + Unpin),
    mut exited: watch::Receiver<bool>,
    group: Option<u32>,
    mut keep: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    // On the heap: a caller awaits several of these at once.
    let mut chunk = vec![0_u8; 8 * 1024];
    let mut child_exited = false;
    loop {
        child_exited |= *exited.borrow();
        let read = if child_exited {
            match timeout(PIPE_IDLE_AFTER_EXIT, pipe.read(&mut chunk)).await {
                Ok(read) => read,
                // Empty all that time: done, unless the group the child led
                // still runs and may still write.
                Err(_) if group.is_none_or(process_group_is_empty) => return Ok(()),
                Err(_) => continue,
            }
        } else {
            tokio::select! {
                read = pipe.read(&mut chunk) => read,
                _ = exited.changed() => {
                    child_exited = true;
                    continue;
                }
            }
        };
        match read {
            Ok(0) => return Ok(()),
            Ok(count) => keep(&chunk[..count])?,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;

    /// Runs `script` as its own group's leader and reads its stdout until
    /// done, or until `limit`.
    async fn run(script: &str, limit: Duration) -> Option<Vec<u8>> {
        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let group = child.id();
        let mut stdout = child.stdout.take().unwrap();
        let mut bytes = Vec::new();
        let (tell, exited) = watch::channel(false);
        let finished = timeout(limit, async {
            tokio::join!(
                wait_and_tell(&mut child, tell),
                read_until_done(&mut stdout, exited, group, |chunk| {
                    bytes.extend_from_slice(chunk);
                    Ok(())
                }),
            )
        })
        .await;
        if let Some(group) = group {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{group}")])
                .status();
        }
        finished.ok().map(|_| bytes)
    }

    #[tokio::test]
    async fn output_ends_with_the_child_though_an_outside_process_holds_the_pipe() {
        // `set -m` puts the lingering job in a group of its own, as a child
        // another thread spawned would be.
        let output = run(
            "set -m; /bin/sleep 30 & echo $!; echo done",
            Duration::from_secs(5),
        )
        .await
        .expect("the read ended with the child");
        let output = String::from_utf8(output).unwrap();
        let (lingering, rest) = output.split_once('\n').unwrap();
        let _ = std::process::Command::new("/bin/kill")
            .arg(lingering)
            .status();
        assert_eq!(rest, "done\n");
    }

    #[tokio::test]
    async fn output_waits_for_the_childs_own_group() {
        let output = run("/bin/sleep 30 & echo started", Duration::from_millis(500)).await;
        assert_eq!(output, None, "a descendant in the group may still write");
    }
}
