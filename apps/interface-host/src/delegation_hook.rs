//! The process-local worker-notice hook. Only the event name crosses the pane
//! socket; no prompt, transcript, command text or file content is copied.
//!
//! `MODE` belonged to pane delegation, retired on 2026-10-14. A session
//! launched before then may still name it; it is accepted and does nothing.
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

pub const MODE: &str = "--interface-delegation-hook";
pub const NOTICE_MODE: &str = "--interface-worker-notice";

#[must_use]
pub fn requested() -> bool {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    arguments.len() == 2
        && (arguments[0] == MODE || arguments[0] == NOTICE_MODE)
        && (arguments[1] == "claude" || arguments[1] == "codex")
}

/// Forward a worker notice for the current pane, if the report workers have one.
///
/// # Errors
/// Returns an error if bounded input or the JSON response cannot be read/written.
pub fn run(input: impl Read, output: impl Write) -> io::Result<()> {
    let notices = std::env::args().nth(1).as_deref() == Some(NOTICE_MODE);
    if !notices || std::env::var_os("INTERFACE_DELEGATE_WORKER").is_some() {
        return Ok(());
    }
    let Some(socket) = std::env::var_os(crate::pane_mcp::PANE_MCP_SOCKET_ENV) else {
        return Ok(());
    };
    report_notice_at(Path::new(&socket), input, output)
}

/// Only the event name is read from a notice hook's payload. It is streamed,
/// so a `PostToolUse` carrying a large tool response is neither buffered nor
/// skipped; serde passes over the other fields without keeping them.
const MAX_NOTICE_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// serde reads a plain reader one byte per call; a buffered one is read in
/// chunks, which matters for multi-megabyte tool responses.
const HOOK_READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
struct NoticeEvent {
    hook_event_name: String,
}

fn report_notice_at(socket: &Path, input: impl Read, mut output: impl Write) -> io::Result<()> {
    if !socket.is_absolute() {
        return Ok(());
    }
    let Ok(NoticeEvent {
        hook_event_name: event,
    }) = serde_json::from_reader(BufReader::with_capacity(
        HOOK_READ_BUFFER_BYTES,
        input.take(MAX_NOTICE_INPUT_BYTES),
    ))
    else {
        return Ok(());
    };
    let event = event.as_str();
    if !matches!(event, "SessionStart" | "UserPromptSubmit" | "PostToolUse") {
        return Ok(());
    }
    // Fail open for notices: never block a turn, tool, edit or Stop. The
    // notice method is stateless, so it skips the initialize round trip.
    let Ok(result) = request_method(socket, false, "interface/worker-notice", &json!({})) else {
        return Ok(());
    };
    if let Some(message) = result["message"].as_str().filter(|s| !s.is_empty()) {
        serde_json::to_writer(
            &mut output,
            &json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":message}}),
        )
        .map_err(io::Error::other)?;
    }
    Ok(())
}

fn read_response(reader: &mut BufReader<UnixStream>) -> io::Result<Value> {
    let mut bytes = Vec::new();
    reader.take(64 * 1024 + 1).read_until(b'\n', &mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::other("oversized hook response"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn request_method(
    socket: &Path,
    initialize: bool,
    method: &str,
    params: &Value,
) -> io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    if initialize {
        writeln!(
            stream,
            "{}",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
        )?;
        read_response(&mut reader)?;
    }
    writeln!(
        stream,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":method,"params":params})
    )?;
    let response = read_response(&mut reader)?;
    if response["result"]["isError"] == true || !response["error"].is_null() {
        return Err(io::Error::other("worker notice refused"));
    }
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .ok_or_else(|| io::Error::other("missing hook response"))?;
    serde_json::from_str(text).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn report_notices_forward_no_prompt_and_never_block_stop() {
        let temp = tempfile::tempdir_in("/private/tmp").unwrap();
        let socket = temp.path().join("notice.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            // No initialize round trip: the notice is the first request.
            let request = read_response(&mut reader).unwrap();
            assert_eq!(request["method"], "interface/worker-notice");
            assert_eq!(request["params"], json!({}));
            let notice = json!({"message":"A worker result is ready.","taskIds":["fixture"]});
            writeln!(stream, "{}", json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":notice.to_string()}]}})).unwrap();
        });
        let mut output = Vec::new();
        // A tool response far over the old 128 KiB read still gets its notice.
        let large = json!({"hook_event_name":"PostToolUse","prompt":"PRIVATE_FIXTURE","tool_input":{"content":"PRIVATE_FIXTURE"},"tool_response":"x".repeat(512 * 1024)}).to_string();
        report_notice_at(&socket, large.as_bytes(), &mut output).unwrap();
        server.join().unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            value["hookSpecificOutput"]["additionalContext"],
            "A worker result is ready."
        );
        assert!(value["decision"].is_null());
        output.clear();
        report_notice_at(
            &socket,
            br#"{"hook_event_name":"Stop"}"#.as_slice(),
            &mut output,
        )
        .unwrap();
        assert!(output.is_empty());
    }
}

#[cfg(test)]
mod disabled_tests {
    use super::*;
    struct Unreadable;
    impl Read for Unreadable {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("the retired delegation hook must not read input or connect")
        }
    }
    #[test]
    fn retired_delegation_hook_returns_without_io_or_instructions() {
        let mut output = Vec::new();
        run(Unreadable, &mut output).unwrap();
        assert!(output.is_empty());
    }
}
